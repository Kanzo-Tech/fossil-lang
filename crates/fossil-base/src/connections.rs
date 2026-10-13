//! The host's connections — `@name/…` → a base URL — as a Salsa input.
//!
//! A location expands through them: the location of a document a program names,
//! of every source it reads, and the run's routing. A map handed beside the
//! queries is memoised with nothing: a check
//! that expanded `@warehouse/…` before the host said what `warehouse` is kept
//! that answer after it did. Read through [`connections`], which registers a
//! dependency, so [`set_connections`] re-runs whatever expanded one.
//!
//! The granularity is the whole map, as the file registry's is ([`crate::files`]):
//! an alias nobody has defined yet is a miss, and only a dependency on the map
//! can express one. A host sets its connections once, from `Host.connections()`,
//! not per keystroke. `/docs/design/incremental` argues it.
//!
//! A registry key never expands through them, so repointing a connection moves
//! where a document is fetched from and not what it is registered under.

use std::collections::BTreeMap;

use crate::db::Db;

/// Connection name → base URL, as a Salsa input.
///
/// A `BTreeMap` for the reason [`crate::files::FileRegistry`] gives: the value
/// is compared on every write to decide whether anything changed.
#[salsa::input(debug)]
pub struct Connections {
    #[returns(ref)]
    pub entries: BTreeMap<String, String>,
}

/// The connections the host has set — empty until it sets any.
#[must_use]
pub fn connections(db: &dyn Db) -> &BTreeMap<String, String> {
    db.connections().entries(db)
}

/// Replace the connections. Setting the map the database already holds
/// changes nothing and re-runs nothing.
pub fn set_connections(db: &mut dyn Db, entries: BTreeMap<String, String>) {
    use salsa::Setter as _;

    let input = db.connections();
    if *input.entries(&*db) == entries {
        return;
    }
    input.set_entries(db).to(entries);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::db::FossilDb;
    use crate::files::SourceFile;
    use crate::system::System;
    use crate::test_support::NativeSystem;

    /// A query that expands an alias, keyed by a file because a tracked
    /// function's key must be a Salsa struct.
    #[salsa::tracked(returns(clone))]
    fn base_of(db: &dyn Db, file: SourceFile) -> Option<String> {
        connections(db).get(file.text(db).as_str()).cloned()
    }

    fn warehouse(base: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("warehouse".to_string(), base.to_string())])
    }

    #[test]
    fn setting_connections_re_runs_what_read_them_and_the_same_map_does_not() {
        let executions = Arc::new(AtomicUsize::new(0));
        let seen = executions.clone();
        let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind
                && format!("{database_key:?}").contains("base_of")
            {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        });
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let mut db = FossilDb::with_event_callback(system, callback);
        let file = SourceFile::new(&db, "warehouse".to_string(), "p.fossil".to_string());

        assert_eq!(base_of(&db, file), None, "no connection is set yet");
        assert_eq!(executions.load(Ordering::SeqCst), 1);

        set_connections(&mut db, warehouse("s3://bucket/"));
        assert_eq!(base_of(&db, file).as_deref(), Some("s3://bucket/"));
        assert_eq!(
            executions.load(Ordering::SeqCst),
            2,
            "the miss was a dependency on the map, so setting it re-ran the query"
        );

        set_connections(&mut db, warehouse("s3://bucket/"));
        assert_eq!(base_of(&db, file).as_deref(), Some("s3://bucket/"));
        assert_eq!(
            executions.load(Ordering::SeqCst),
            2,
            "the same map again is no write"
        );
    }
}
