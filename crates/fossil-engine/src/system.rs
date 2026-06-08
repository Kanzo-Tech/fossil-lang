//! The engine's [`System`] impl — a native host providing fs reads + the
//! host-registered [`InferredDescriptor`] table the bidirectional checker reads.

// These items are `pub(crate)` (private module ⇒ unreachable_pub wants pub(crate));
// that trips the inverse `redundant_pub_crate` nursery lint, silenced here — the
// same convention the rest of the codebase uses.
#![allow(clippy::redundant_pub_crate)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use fossil_base::{FsError, System};
use fossil_descriptors_input::InferredDescriptor;
use fossil_descriptors_output::SystemWithDescriptors;
use smol_str::SmolStr;

/// Native [`System`] for the engine. ALSO implements [`SystemWithDescriptors`] so
/// the bidirectional checker reaches the output-descriptor accessor via the
/// extension trait (Option B from plan 03-03; see ADR-0006). The default
/// `output_descriptor_kind()` returns `ACCEPT_ALL_DEFAULT`.
#[derive(Debug, Default)]
pub(crate) struct EngineSystem {
    /// Phase 13 INPUT-01 (ADR-0037) — host-registered `InferredDescriptor`s keyed
    /// by source binding name (e.g. `"users"` for `users := io.csv(...)`), populated
    /// by `introspect::pre_introspect_and_register` ahead of every compile. `Mutex`
    /// is right: writes happen once per compile, reads are per-mapping.
    inferred: Mutex<HashMap<SmolStr, InferredDescriptor>>,
}

impl System for EngineSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound(path.display().to_string()),
            _ => FsError::Io(e.to_string()),
        })
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn inferred_descriptor(&self, source_name: &str) -> Option<InferredDescriptor> {
        self.inferred.lock().ok()?.get(source_name).cloned()
    }

    fn register_inferred_descriptor(&self, descriptor: InferredDescriptor) {
        if let Ok(mut lock) = self.inferred.lock() {
            lock.insert(descriptor.source_name.clone(), descriptor);
        }
    }
}

impl SystemWithDescriptors for EngineSystem {
    // Default impl returns AcceptAll — see `SystemWithDescriptors`.
}

/// Build a fresh `FossilDb` over the engine [`System`] for `path` + `text`.
pub(crate) fn open_db(text: String, path: &Path) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
    let system: Arc<dyn System> = Arc::new(EngineSystem::default());
    let db = fossil_base::FossilDb::new(system);
    let file = fossil_base::SourceFile::new(&db, text, path.to_string_lossy().into_owned());
    (db, file)
}
