//! Salsa `Db` trait + `FossilDb` implementation.
//!
//! The trait is intentionally **thin**: `system()`, and one handle per input a
//! host writes — `files()`, `sources()`, `connections()`, `catalogue()`. This is the pattern
//! from `ruff_db` (and `ty_wasm` composes `Workspace { db, system }` over it):
//! what changes a compile's answer is an input, never ambient host state.
//!
//! The registry does NOT flow through `System`: a `&'static [&'static Provider]`
//! is a table nothing can invalidate and no file can produce. It is a Salsa
//! input, reached the same way [`Files`] is.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use crate::connections::Connections;
use crate::files::Files;
use crate::providers::Catalogue;
use crate::sources::Sources;
use crate::system::System;

#[salsa::db]
pub trait Db: salsa::Database {
    fn system(&self) -> &dyn System;
    fn files(&self) -> &Files;
    /// What each source a program reads is — see [`crate::sources::described`].
    fn sources(&self) -> &Sources;
    /// The host's connections — see [`crate::connections::connections`].
    fn connections(&self) -> Connections;
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
    sources: Sources,
    connections: OnceLock<Connections>,
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

    /// The one constructor. It exists because every handle on [`Db`] is a Salsa
    /// input — the file registry ([`crate::files::FileRegistry`]), the sources,
    /// the connections and the provider catalogue ([`crate::providers::Registry`])
    /// — and a Salsa input can only be created with a database in hand — so the
    /// database is built first and each forced immediately after, **outside any query**. Salsa does not
    /// stop a query body from creating an input, but an input a query creates is
    /// not in that query's dependency list; allocating here means no query ever
    /// has to.
    fn with_storage(storage: salsa::Storage<Self>, system: Arc<dyn System>) -> Self {
        let db = Self {
            storage,
            system,
            files: Files::default(),
            sources: Sources::default(),
            connections: OnceLock::new(),
            catalogue: Catalogue::default(),
        };
        let _ = db.files.registry(&db);
        let _ = db.sources.registry(&db);
        let _ = db.connections();
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

    fn sources(&self) -> &Sources {
        &self.sources
    }

    fn connections(&self) -> Connections {
        *self
            .connections
            .get_or_init(|| Connections::new(self, BTreeMap::new()))
    }

    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::SourceFile;
    use crate::test_support::NativeSystem;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Tracked helper that DOES fire a `WillExecute` event so the smoke test
    /// can prove the registered callback is actually wired into the runtime.
    /// Salsa input creation by itself does not emit a `WillExecute`; only
    /// `#[salsa::tracked]` functions do. This helper is the minimum surface
    /// that proves end-to-end wiring; `fossil-hir`'s invalidation regression
    /// test builds the full count atop the same constructor.
    #[salsa::tracked(returns(clone))]
    fn _read_text(db: &dyn Db, file: SourceFile) -> String {
        file.text(db).clone()
    }

    #[test]
    fn with_event_callback_constructs_a_db_and_fires_at_least_one_event() {
        let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |_evt| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::with_event_callback(system, callback);
        let file = SourceFile::new(&db, "x".to_string(), "x.fossil".to_string());
        let _ = _read_text(&db, file);

        assert!(
            counter.load(Ordering::SeqCst) > 0,
            "no Salsa events fired through the registered callback"
        );
    }
}
