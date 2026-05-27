//! [`WasmSystem`] — in-memory [`fossil_base::System`] impl for the WASM host.
//!
//! Per RESEARCH.md Pitfall 4, `SystemTime::now()` panics on
//! `wasm32-unknown-unknown` (the upstream `std` impl unconditionally calls
//! `__wasi_clock_time_get` which is unavailable in the bare wasm target).
//! Phase 1 has zero callers of `Db::system().now()` on the compile path, so
//! [`WasmSystem::now`] returns [`SystemTime::UNIX_EPOCH`] as a safe placeholder.
//! Phase 7+ replaces this with the `web-time` crate (a `SystemTime`-shaped
//! shim that delegates to `performance.now()` in the browser and
//! `process.hrtime` in Node).
//!
//! The filesystem is a programmable in-memory map: hosts can stage source
//! files via [`WasmSystem::write`] before invoking compiler queries that need
//! to read them. Phase 1 [`crate::FossilPlayground::compile`] does NOT use
//! this — `compile(source: &str)` ingests the source string directly via
//! [`fossil_base::SourceFile::new`] — so the map stays empty during the Phase
//! 1 smoke test. The setter exists so Phase 7's `open_file/update_file/
//! close_file` lifecycle has a place to land without reshaping the system
//! trait.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

use fossil_base::{FsError, System};
use fossil_descriptors_input::InferredDescriptor;
use fossil_descriptors_output::SystemWithDescriptors;
use smol_str::SmolStr;

// `pub(crate)` is the deliberate visibility: `WasmSystem` is an
// implementation detail of the `fossil-wasm` crate. The clippy
// `redundant_pub_crate` nursery lint suggests `pub` (since the parent
// module is private), but rustc's `unreachable_pub` lint then complains the
// other way. We pick `pub(crate)` so that adding a sibling submodule (e.g.
// Phase 7 PLAY-01's `lifecycle.rs`) doesn't require rewiring visibility,
// and silence the conflicting clippy advice locally.
#[allow(clippy::redundant_pub_crate)]
#[derive(Debug, Default)]
pub(crate) struct WasmSystem {
    fs: RwLock<HashMap<String, Vec<u8>>>,
    /// Phase 13 INPUT-01 (ADR-0037): host-registered `InferredDescriptors`,
    /// keyed by source binding name (e.g. `"users"` for `users := io.csv(...)`).
    /// Populated by the playground orchestration via
    /// [`crate::FossilPlayground::register_inferred_descriptor`] BEFORE
    /// invoking `compile()` / `compile_file()`. Consumed by
    /// `fossil-hir::infer::resolve_source_row` (after 13-02 ships) inside
    /// the typecheck tracked query.
    ///
    /// `Mutex` (not `RwLock`) — write traffic is rare (per-compile, once per
    /// source binding) and reads are cheap clones (per-mapping); the
    /// `RwLock` overhead isn't justified for this access pattern. Mirrors
    /// the `NativeSystem` choice in `fossil-base::system::NativeSystem`.
    inferred: Mutex<HashMap<SmolStr, InferredDescriptor>>,
}

impl System for WasmSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        let key = path.to_string_lossy().into_owned();
        let bytes = self
            .fs
            .read()
            .expect("WasmSystem fs lock poisoned")
            .get(&key)
            .cloned();
        bytes.ok_or(FsError::NotFound(key))
    }

    fn now(&self) -> SystemTime {
        // Phase 1: no callers; UNIX_EPOCH avoids the wasm32 SystemTime::now()
        // panic documented in RESEARCH.md Pitfall 4. Phase 7+ adds the
        // `web-time` crate.
        SystemTime::UNIX_EPOCH
    }

    fn inferred_descriptor(&self, source_name: &str) -> Option<InferredDescriptor> {
        // Lock can only fail if poisoned (another thread panicked while
        // holding it). Treat that as "no descriptor available" — the
        // typecheck fallback path handles missing descriptors gracefully.
        self.inferred.lock().ok()?.get(source_name).cloned()
    }

    fn register_inferred_descriptor(&self, descriptor: InferredDescriptor) {
        if let Ok(mut lock) = self.inferred.lock() {
            lock.insert(descriptor.source_name.clone(), descriptor);
        }
    }
}

/// Plan 03-03 Task 2 step 6: WASM host wires `SystemWithDescriptors` impl
/// on its concrete `WasmSystem` struct. Default `output_descriptor_kind()`
/// returns `AcceptAll` for Phase 3 v0.1 — plan 03-05's tests construct
/// `ShExDescriptor` directly. Phase 7 plan 07-02 (this commit) wires the
/// user-supplied `ShEx` text via [`crate::FossilPlayground::set_target_shex`]
/// — but the swappable storage lives on the host's `WasmDb`
/// (`Arc<OutputDescriptorKind>`, mirroring `LspDb`), not on the `System`
/// trait object. The accessor is on `HirDb`, not `Db::system()` (ADR-0020).
impl SystemWithDescriptors for WasmSystem {}

#[allow(clippy::redundant_pub_crate)]
impl WasmSystem {
    /// Programmatic file write — used by hosts (Phase 7 PLAY-01 calls this
    /// from `FossilPlayground::open_file`). Phase 1 has no callers; the
    /// method exists for completeness as the symmetric setter to
    /// [`System::read_file`] and to anchor the symbol so Phase 7's lifecycle
    /// API doesn't have to reshape this module.
    #[allow(dead_code)] // Phase 7 PLAY-01 wires the first caller
    pub(crate) fn write(&self, path: &str, contents: Vec<u8>) {
        self.fs
            .write()
            .expect("WasmSystem fs lock poisoned")
            .insert(path.to_string(), contents);
    }
}
