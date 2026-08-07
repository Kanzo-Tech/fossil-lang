//! The engine's [`System`] impl — a native host providing fs reads + the
//! host-introspected descriptor table the bidirectional checker reads.

// These items are `pub(crate)` (private module ⇒ unreachable_pub wants pub(crate));
// that trips the inverse `redundant_pub_crate` nursery lint, silenced here — the
// same convention the rest of the codebase uses.
#![allow(clippy::redundant_pub_crate)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use fossil_base::{FsError, System};
use fossil_descriptors_input::DescriptorCache;
use fossil_descriptors_output::SystemWithDescriptors;

/// The descriptor cache for programs living in `program_dir`, created on first
/// sight of that directory and shared by every compile of every program in it.
///
/// **Why it is not on the db.** [`open_db`] builds a fresh
/// [`fossil_base::FossilDb`] with empty `Storage` on every call — the finding
/// in ADR-0046 §6 — so a table hanging off the db is thrown away between the
/// `check` and the `run` that follows it, which is precisely the pair we want
/// to introspect once. The cache is ambient instead, which is also where
/// ADR-0046 §5 puts an extension table.
///
/// **Why it is per-directory and not per-process.** The cache is keyed by the
/// URI as the program writes it (ADR-0050), and that URI is usually relative:
/// `"users.csv"` names a different file in two different directories. The
/// program's directory is the scope in which a written URI is unambiguous, so
/// it is the scope of the table. Entries are never evicted; a long-lived host
/// holds one table per program directory it has ever compiled, and the
/// freshness token — not eviction — is what keeps an entry truthful.
fn descriptor_cache(program_dir: &Path) -> Arc<DescriptorCache> {
    static CACHES: OnceLock<Mutex<HashMap<PathBuf, Arc<DescriptorCache>>>> = OnceLock::new();
    let caches = CACHES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut caches) = caches.lock() else {
        // A poisoned lock means another thread panicked mid-registration. A
        // private table costs one extra introspection and no wrong answers,
        // which beats failing the compile.
        return Arc::new(DescriptorCache::new());
    };
    Arc::clone(
        caches
            .entry(program_dir.to_path_buf())
            .or_insert_with(|| Arc::new(DescriptorCache::new())),
    )
}

/// Native [`System`] for the engine. ALSO implements [`SystemWithDescriptors`] so
/// the bidirectional checker reaches the output-descriptor accessor via the
/// extension trait (Option B from plan 03-03; see ADR-0006). The default
/// `output_descriptor_kind()` returns `ACCEPT_ALL_DEFAULT`.
#[derive(Debug)]
pub(crate) struct EngineSystem {
    descriptors: Arc<DescriptorCache>,
}

impl EngineSystem {
    /// A `System` for a program that lives in `program_dir`, sharing that
    /// directory's [`DescriptorCache`] with every other compile there.
    pub(crate) fn for_program_dir(program_dir: &Path) -> Self {
        Self {
            descriptors: descriptor_cache(program_dir),
        }
    }
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

    fn descriptors(&self) -> Option<&DescriptorCache> {
        Some(&self.descriptors)
    }
}

impl SystemWithDescriptors for EngineSystem {
    // Default impl returns AcceptAll — see `SystemWithDescriptors`.
}

/// Build a fresh `FossilDb` over the engine [`System`] for `path` + `text`.
pub(crate) fn open_db(
    text: String,
    path: &Path,
) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
    let program_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let system: Arc<dyn System> = Arc::new(EngineSystem::for_program_dir(program_dir));
    let db = fossil_base::FossilDb::new(system);
    let file = fossil_base::SourceFile::new(&db, text, path.to_string_lossy().into_owned());
    (db, file)
}
