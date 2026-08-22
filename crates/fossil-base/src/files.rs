//! File registry + Salsa source-file input.
//!
//! [`SourceFile`] is the compiler's one source of text: a Salsa **input**, so
//! reading `text(db)` from a tracked query registers a dependency and editing it
//! invalidates every reader.
//!
//! [`Files`] is the set of files the compiler may read, keyed by **resolved path
//! string**. It was an empty placeholder whose own doc said it becomes this
//! registry "once multi-file resolution is needed"; a shape document named by
//! `io.shex("shop.shex")` is a second file, so it is needed now.
//!
//! # Why the registry is itself a Salsa input
//!
//! The path→file map has to be a [`FileRegistry`] input rather than a plain
//! `HashMap` on the side, and the reason is a *miss*, not a hit. `fossil-hir`
//! resolves a path and reads the bytes through `System::read_file`, which
//! registers nothing — so a query that asked for `shop.shex` before it existed
//! memoized "not found" and **never re-ran when it appeared**. In an LSP that is
//! a diagnostic that never clears. [`file_at`] reads the map through the input's
//! getter, so the *absence* of a path is a dependency on the map exactly as much
//! as its presence is: registering a new file bumps the field's revision, and
//! every reader that previously missed re-executes.
//!
//! The granularity is the whole map, deliberately. Registering any file
//! invalidates every [`file_at`] reader, including those that found a different
//! file. A per-path input would be finer, but it cannot express a miss — that is
//! the entire bug — and registration happens at host setup and on file-open, not
//! per keystroke. Text edits, which *are* per-keystroke, go through
//! [`SourceFile::set_text`] and touch nothing here.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::db::Db;

#[salsa::input(debug)]
pub struct SourceFile {
    #[returns(ref)]
    pub text: String,
    #[returns(ref)]
    pub path: String,
}

/// The path→file map, as a Salsa input so that reading it registers a
/// dependency. See the module docs for why a miss has to be tracked.
///
/// A `BTreeMap` rather than a `HashMap`: this value is compared by
/// `PartialEq` on every write to decide whether anything actually changed, and
/// it is small. Ordering it also means a future consumer that iterates gets the
/// file order rather than the process's hash seed — the failure
/// `crates/fossil-shex/examples/declaration_order.rs` already measured once.
#[salsa::input(debug)]
pub struct FileRegistry {
    #[returns(ref)]
    pub entries: BTreeMap<String, SourceFile>,
}

/// The set of files the compiler may read.
///
/// Held by the `Db` implementation and handed out by `Db::files`. The struct
/// itself is a handle: the state is the [`FileRegistry`] input, allocated in the
/// database on first use and then never replaced.
#[derive(Debug, Default, Clone)]
pub struct Files {
    /// Allocated once, lazily, because a Salsa input can only be created with a
    /// database in hand and `Files::default()` has none. `FossilDb::new` forces
    /// it immediately so the ordinary path allocates outside any query;
    /// [`Files::registry`] is still correct if a foreign `Db` implementation
    /// does not, because an empty registry allocated mid-revision is read at
    /// that revision and any later registration bumps it.
    registry: OnceLock<FileRegistry>,
}

impl Files {
    /// The registry input, allocating it on first use.
    ///
    /// Prefer forcing this from the database constructor: salsa 0.26 lets an
    /// input be created while a query runs — nothing checks — but the created
    /// input is invisible to the running query's dependency list, so an input
    /// created *and read* inside one query body is an untracked read. This one
    /// is safe from that because it is created empty and read through
    /// [`FileRegistry::entries`], which does register; but the general rule
    /// stands and the API is shaped for it: hosts register files **before** the
    /// queries that look for them run, and registration needs `&mut` anyway,
    /// which no query body can have.
    pub fn registry<D>(&self, db: &D) -> FileRegistry
    where
        D: ?Sized + salsa::Database,
    {
        *self
            .registry
            .get_or_init(|| FileRegistry::new(db, BTreeMap::new()))
    }
}

/// The [`SourceFile`] registered at `path`, or `None`.
///
/// **The read registers a Salsa dependency, and that is the point.** A `None`
/// from here is not a cached dead end: it is a dependency on the registry's
/// contents, so [`register_file`] invalidates the readers that previously
/// missed. Resolving a path with `System::read_file` instead — what the compiler
/// did until now — memoized the miss forever.
#[must_use]
pub fn file_at(db: &dyn Db, path: &str) -> Option<SourceFile> {
    db.files().registry(db).entries(db).get(path).copied()
}

/// Register `file` under its resolved `path`, replacing any previous entry.
///
/// Takes `&mut dyn Db` because writing a Salsa input takes the database
/// exclusively — which is also what makes "register before you query" the only
/// expressible order.
pub fn register_file(db: &mut dyn Db, path: String, file: SourceFile) {
    use salsa::Setter as _;

    let registry = db.files().registry(&*db);
    let mut entries = registry.entries(&*db).clone();
    entries.insert(path, file);
    registry.set_entries(db).to(entries);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::db::FossilDb;
    use crate::system::System;
    use crate::test_support::NativeSystem;

    /// A tracked query's key must be a Salsa struct — salsa 0.26 rejects a bare
    /// `String` with "the trait bound `String: SalsaStructInDb` is not
    /// satisfied". So a query that wants to be keyed by a *path* interns it.
    /// [`file_at`] itself takes a `&str` precisely so it can be called from a
    /// query keyed by anything at all.
    #[salsa::interned(debug)]
    struct ResolvedPath {
        #[returns(ref)]
        path: String,
    }

    #[salsa::tracked]
    fn text_at<'db>(db: &'db dyn Db, path: ResolvedPath<'db>) -> Option<String> {
        file_at(db, path.path(db)).map(|f| f.text(db).clone())
    }

    fn db() -> FossilDb {
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        FossilDb::new(system)
    }

    #[test]
    fn an_unregistered_path_is_none_and_a_registered_one_is_found() {
        let mut db = db();
        assert!(file_at(&db, "shop.shex").is_none());

        let f = SourceFile::new(&db, "PREFIX ex:".to_string(), "shop.shex".to_string());
        register_file(&mut db, "shop.shex".to_string(), f);

        assert_eq!(file_at(&db, "shop.shex"), Some(f));
        assert!(file_at(&db, "other.shex").is_none());
    }

    /// The bug this registry exists to remove: a query that missed must
    /// re-execute once the file it was looking for is registered. With
    /// `System::read_file` the miss was memoized and nothing ever invalidated
    /// it.
    #[test]
    fn registering_a_file_invalidates_the_readers_that_missed_it() {
        let executions: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        let seen = executions.clone();
        let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind
                && format!("{database_key:?}").contains("text_at")
            {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        });
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let mut db = FossilDb::with_event_callback(system, callback);

        assert_eq!(
            text_at(&db, ResolvedPath::new(&db, "shop.shex".to_string())),
            None
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1, "cold miss");

        assert_eq!(
            text_at(&db, ResolvedPath::new(&db, "shop.shex".to_string())),
            None
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1, "the miss is cached");

        let f = SourceFile::new(&db, "ex:Person {}".to_string(), "shop.shex".to_string());
        register_file(&mut db, "shop.shex".to_string(), f);

        assert_eq!(
            text_at(&db, ResolvedPath::new(&db, "shop.shex".to_string())),
            Some("ex:Person {}".to_string()),
            "the reader that missed now sees the file"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            2,
            "registering re-executed it — a cached None that never invalidates \
             is the bug this registry removes"
        );
    }

    #[test]
    fn the_registry_is_allocated_once_and_survives_registration() {
        let mut db = db();
        let first = db.files().registry(&db);
        let f = SourceFile::new(&db, "x".to_string(), "a".to_string());
        register_file(&mut db, "a".to_string(), f);
        assert_eq!(
            db.files().registry(&db),
            first,
            "registration sets the field; it does not allocate a new input"
        );
    }
}
