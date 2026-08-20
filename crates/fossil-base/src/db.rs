//! Salsa `Db` trait + `FossilDb` implementation.
//!
//! The trait is intentionally **thin**: `system()`, `files()` and
//! `catalogue()`. Descriptors and host capabilities flow through `&dyn System`
//! rather than as separate composing traits. This is the verified pattern from
//! `ruff_db` (and `ty_wasm` composes `Workspace { db, system }` over it).
//!
//! The registry does NOT flow through `System`, and used to. `System::providers`
//! returned `&'static [&'static Provider]`, which is a table nothing can
//! invalidate and no file can produce; it is a Salsa input now, reached the same
//! way [`Files`] is.

use std::sync::Arc;

use crate::files::Files;
use crate::providers::Catalogue;
use crate::system::System;

#[salsa::db]
pub trait Db: salsa::Database {
    fn system(&self) -> &dyn System;
    fn files(&self) -> &Files;
    /// The provider rows installed in this database — see
    /// [`crate::providers::installed`], which is how a query reads them.
    fn catalogue(&self) -> &Catalogue;
}

#[salsa::db]
#[derive(Clone)]
pub struct FossilDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
    catalogue: Catalogue,
}

impl std::fmt::Debug for FossilDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FossilDb")
            .field("system", &self.system)
            .finish_non_exhaustive()
    }
}

impl FossilDb {
    #[must_use]
    pub fn new(system: Arc<dyn System>) -> Self {
        Self::with_storage(salsa::Storage::default(), system)
    }

    /// Construct a `FossilDb` whose Salsa runtime invokes `callback` for
    /// every `salsa::Event` emitted during query execution.
    ///
    /// Used by the invalidation regression test
    /// (`crates/fossil-hir/tests/invalidation_regression.rs`) to count
    /// `EventKind::WillExecute` events across an edit-trigger boundary, which
    /// is how the cap on re-executed queries after a body-only edit in one of
    /// ten mappings is enforced. That test owns the threshold and the reasoning
    /// for it; this constructor exists so it can observe the events at all.
    ///
    /// `FossilDb::new` remains the no-callback variant for everyday use.
    #[must_use]
    pub fn with_event_callback(
        system: Arc<dyn System>,
        callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static>,
    ) -> Self {
        Self::with_storage(salsa::Storage::new(Some(callback)), system)
    }

    /// The one constructor. It exists because the file registry
    /// ([`crate::files::FileRegistry`]) and the provider catalogue
    /// ([`crate::providers::Registry`]) are Salsa inputs, and a Salsa input can
    /// only be created with a database in hand — so the database is built first
    /// and both forced immediately after, **outside any query**. Salsa does not
    /// stop a query body from creating an input, but an input a query creates is
    /// not in that query's dependency list; allocating here means no query ever
    /// has to.
    fn with_storage(storage: salsa::Storage<Self>, system: Arc<dyn System>) -> Self {
        let db = Self {
            storage,
            system,
            files: Files::default(),
            catalogue: Catalogue::default(),
        };
        let _ = db.files.registry(&db);
        // The host DECLARES its table through `System::providers`; the
        // catalogue is what everything READS, and it seeds itself from that
        // declaration. Forcing it here keeps the allocation outside any query.
        let _ = db.catalogue.registry(&db);
        db
    }
}

#[salsa::db]
impl salsa::Database for FossilDb {}

#[salsa::db]
impl Db for FossilDb {
    fn system(&self) -> &dyn System {
        &*self.system
    }

    fn files(&self) -> &Files {
        &self.files
    }

    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::files::SourceFile;
    use crate::system::NativeSystem;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Tracked helper that DOES fire a `WillExecute` event so the smoke test
    /// can prove the registered callback is actually wired into the runtime.
    /// Salsa input creation by itself does not emit a `WillExecute`; only
    /// `#[salsa::tracked]` functions do. This helper is the minimum surface
    /// that proves end-to-end wiring; `fossil-hir`'s invalidation regression
    /// test builds the full count atop the same constructor.
    #[salsa::tracked]
    fn _read_text(db: &dyn Db, file: SourceFile) -> String {
        file.text(db).to_string()
    }

    #[test]
    fn with_event_callback_constructs_a_db_and_fires_at_least_one_event() {
        // Counter captured by the callback — the test asserts the Salsa
        // runtime invoked the callback at least once during query execution.
        let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |_evt| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::with_event_callback(system, callback);
        let file = SourceFile::new(&db, "x".to_string(), "x.fossil".to_string());
        // Executing a tracked function flushes a `WillExecute` event through
        // the registered callback.
        let _ = _read_text(&db, file);

        assert!(
            counter.load(Ordering::SeqCst) > 0,
            "no Salsa events fired through the registered callback"
        );
    }
}
