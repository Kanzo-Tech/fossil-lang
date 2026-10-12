//! What each source a program reads is — a Salsa input the host writes.
//!
//! A host profiles the sources a program names (`fossil-profile`, on its own
//! engine) and writes the answer here, one entry per source location as the
//! program writes it: the columns it found, or the problem that stopped it.
//! The checker reads an entry through [`described`], which registers a
//! dependency, so describing a source re-runs every check that read it.
//!
//! The granularity is the whole map, for the reason [`crate::files`] gives for
//! the file registry: a check that asked for a source nobody had described yet
//! has to re-run when one is, and only a dependency on the map can express a
//! miss. Hosts describe sources after a profile, not per keystroke.
//! `/docs/design/incremental` argues it and says what would reverse it.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use fossil_descriptors_input::InferredDescriptor;
use fossil_graph_schema::Problem;
use smol_str::SmolStr;

use crate::db::Db;

/// What the host found when it read a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Described {
    /// The columns and their types.
    Columns(InferredDescriptor),
    /// The source could not be read, and why: a warning at the binding that
    /// reads it, under the problem's own code, and an open row for the check.
    Refused {
        problem: Problem,
        help: Option<String>,
    },
}

/// The location → description map, as a Salsa input.
#[salsa::input(debug)]
pub struct SourceRegistry {
    #[returns(ref)]
    pub entries: BTreeMap<SmolStr, Described>,
}

/// The handle a `Db` holds: the [`SourceRegistry`] input, allocated once.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    registry: OnceLock<SourceRegistry>,
}

impl Sources {
    /// The registry input, allocating it on first use. A database constructor
    /// forces it, so no query is the one that allocates it.
    pub fn registry<D>(&self, db: &D) -> SourceRegistry
    where
        D: ?Sized + salsa::Database,
    {
        *self
            .registry
            .get_or_init(|| SourceRegistry::new(db, BTreeMap::new()))
    }
}

/// What the host found for the source at `location`, if it has said.
#[must_use]
pub fn described(db: &dyn Db, location: &str) -> Option<Described> {
    db.sources().registry(db).entries(db).get(location).cloned()
}

/// The columns the host found for `location` — `None` for a source not yet
/// described, or one the host could not read.
#[must_use]
pub fn columns(db: &dyn Db, location: &str) -> Option<InferredDescriptor> {
    match described(db, location)? {
        Described::Columns(descriptor) => Some(descriptor),
        Described::Refused { .. } => None,
    }
}

/// Record what the host found for `location`, replacing what it said before.
/// Writing the same description again changes nothing and re-runs nothing.
pub fn describe(db: &mut dyn Db, location: &str, description: Described) {
    use salsa::Setter as _;

    let registry = db.sources().registry(&*db);
    if registry.entries(&*db).get(location) == Some(&description) {
        return;
    }
    let mut entries = registry.entries(&*db).clone();
    entries.insert(SmolStr::new(location), description);
    registry.set_entries(db).to(entries);
}

/// Record a source's columns under the location the descriptor carries.
pub fn describe_columns(db: &mut dyn Db, descriptor: InferredDescriptor) {
    let location = descriptor.key.clone();
    describe(db, &location, Described::Columns(descriptor));
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
    use fossil_graph_schema::Primitive;

    use super::*;
    use crate::db::FossilDb;
    use crate::test_support::NativeSystem;

    fn sample(location: &str) -> InferredDescriptor {
        InferredDescriptor {
            key: location.into(),
            columns: vec![InferredColumn {
                name: "id".into(),
                primitive: Primitive::Integer,
            }],
            etag: String::new(),
        }
    }

    #[test]
    fn a_described_source_reads_back_and_an_undescribed_one_is_none() {
        let mut db = FossilDb::new(Arc::new(NativeSystem::default()));
        describe_columns(&mut db, sample("users.csv"));
        assert_eq!(columns(&db, "users.csv"), Some(sample("users.csv")));
        assert!(described(&db, "nope.csv").is_none());
    }

    #[test]
    fn a_refused_source_has_no_columns() {
        let mut db = FossilDb::new(Arc::new(NativeSystem::default()));
        let problem = Problem::SourceNotFound {
            location: "users.csv".into(),
        };
        describe(
            &mut db,
            "users.csv",
            Described::Refused {
                problem,
                help: None,
            },
        );
        assert!(described(&db, "users.csv").is_some());
        assert!(columns(&db, "users.csv").is_none());
    }
}
