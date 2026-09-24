//! [`WasmSystem`] — in-memory [`fossil_base::System`] impl for the WASM host.
//!
//! `SystemTime::now()` panics on
//! `wasm32-unknown-unknown` (the upstream `std` impl unconditionally calls
//! `__wasi_clock_time_get` which is unavailable in the bare wasm target).
//! Nothing on the compile path calls `Db::system().now()`, so
//! [`WasmSystem::now`] returns [`SystemTime::UNIX_EPOCH`] as a safe placeholder.
//! The first caller that needs a real clock pulls in the `web-time` crate — a
//! `SystemTime`-shaped shim that delegates to `performance.now()` in the
//! browser and `process.hrtime` in Node — rather than making this method fail.
//!
//! There is no filesystem. A buffer arrives through `open_file` and a document
//! through `register_document`, both into the Salsa file registry, so
//! [`WasmSystem::read_file`] has nothing to answer.

use std::path::Path;
use std::time::SystemTime;

use fossil_base::{FsError, Provider, System};
use fossil_descriptors_input::DescriptorCache;

// `pub(crate)` is the deliberate visibility: `WasmSystem` is an
// implementation detail of the `fossil-wasm` crate. The clippy
// `redundant_pub_crate` nursery lint suggests `pub` (since the parent
// module is private), but rustc's `unreachable_pub` lint then complains the
// other way. We pick `pub(crate)` so that adding a sibling submodule doesn't
// require rewiring visibility, and silence the conflicting clippy advice
// locally.
#[allow(clippy::redundant_pub_crate)]
#[derive(Debug, Default)]
pub(crate) struct WasmSystem {
    /// Keyed by the source URI the program writes:
    /// the descriptors the playground introspected with DuckDB-WASM and
    /// pushed in via [`crate::FossilPlayground::register_inferred_descriptor`]
    /// BEFORE invoking `check()`. Consumed by
    /// `fossil-hir::infer::resolve_source_scope` inside the typecheck query.
    descriptors: DescriptorCache,
}

impl System for WasmSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound(path.display().to_string()))
    }

    fn now(&self) -> SystemTime {
        // No callers; UNIX_EPOCH avoids the wasm32 SystemTime::now() panic.
        // The first caller that needs a real clock adds `web-time`.
        SystemTime::UNIX_EPOCH
    }

    fn descriptors(&self) -> Option<&DescriptorCache> {
        Some(&self.descriptors)
    }

    /// The playground CHECKS programs — it is the LSP server-side in the
    /// browser — so it installs the same rows the native host
    /// does. A host with no rows checks every program against no output
    /// contract, which would make the editor's target-side hover and
    /// completion silently empty for exactly the programs that declare a
    /// shape.
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}
