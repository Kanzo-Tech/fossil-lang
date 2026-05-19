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
//! 1 smoke test. The setter exists so Phase 7 PLAY-01's
//! `open_file/update_file/close_file` lifecycle has a place to land without
//! reshaping the system trait.

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;
use std::time::SystemTime;

use fossil_base::{FsError, System};
use fossil_descriptors_output::SystemWithDescriptors;

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
}

/// Plan 03-03 Task 2 step 6: WASM host wires `SystemWithDescriptors` impl
/// on its concrete `WasmSystem` struct. Default `output_descriptor_kind()`
/// returns `AcceptAll` for Phase 3 v0.1 — plan 03-05's tests construct
/// `ShExDescriptor` directly. Phase 7+ playground wires user-supplied
/// `ShEx` text via a future `FossilPlayground::load_shex(...)` method.
impl SystemWithDescriptors for WasmSystem {}

impl WasmSystem {
    /// Programmatic file write — used by hosts (Phase 7 PLAY-01 calls this
    /// from `FossilPlayground::open_file`). Phase 1 has no callers; the
    /// method exists for completeness as the symmetric setter to
    /// [`System::read_file`] and to anchor the symbol so Phase 7's lifecycle
    /// API doesn't have to reshape this module.
    #[allow(dead_code, clippy::redundant_pub_crate)] // Phase 7 PLAY-01 wires the first caller
    pub(crate) fn write(&self, path: &str, contents: Vec<u8>) {
        self.fs
            .write()
            .expect("WasmSystem fs lock poisoned")
            .insert(path.to_string(), contents);
    }
}
