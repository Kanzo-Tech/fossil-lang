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

use fossil_graph_schema::{Failure, Primitive, Problem};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::db::Db;

/// One column from a host-introspected source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InferredColumn {
    /// Column name as introspected (`DuckDB` `column_name`).
    #[schemars(with = "String")]
    pub name: SmolStr,
    /// The column's place in the lattice — the same enum the checker types
    /// against, so a host sends `"integer"`, not a name the consumer has to
    /// look up. A value outside the lattice fails here, at deserialisation,
    /// naming itself; it does not become a `String` column and a diagnostic
    /// three crates away.
    pub primitive: Primitive,
}

/// A descriptor inferred from a runtime file introspection.
///
/// Identified by the source **URI**, which is what the descriptor is about: a
/// binding name is a name for the *program*'s convenience, and two of them can
/// point at one file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InferredDescriptor {
    /// The source URI exactly as written in the program — the string inside
    /// `io.csv("examples/users.csv")`. NOT the resolved location: the host
    /// resolves (`@conn` aliases, a program-relative path, a signed URL) in
    /// order to *read* the source, but the checker only ever sees what the
    /// program says, so that is the only string both ends can agree on.
    #[schemars(with = "String")]
    pub key: SmolStr,
    /// Ordered columns; the order is the source's own and is significant.
    pub columns: Vec<InferredColumn>,
    /// An entity tag in RFC 9110's sense (§8.8.3): an opaque validator of the
    /// state of the source this was read from, which may be weak. A host compares
    /// it to decide whether to profile the source again; nothing interprets it. A host that can stat a file writes
    /// `mtime` + size, a host that has a strong `ETag` or a content digest
    /// writes that instead, and a host that cannot cheaply tell writes `""`, which
    /// is never fresh, so that source is profiled every time. It is NOT a hash of the bytes,
    /// and deliberately: hashing means reading the whole source to decide
    /// whether the source needs reading, which makes the cache cost more than
    /// the `DESCRIBE` it saves. `mtime` + size is two fields of one `stat`. Its
    /// one dangerous failure — saying "unchanged" when it changed — needs a file
    /// restored with the same `mtime` AND the same size, which is what pairing
    /// the two narrows.
    pub etag: String,
}

impl InferredDescriptor {
    /// Build an empty descriptor for `key` — useful for tests.
    #[must_use]
    pub fn empty(key: impl Into<SmolStr>) -> Self {
        Self {
            key: key.into(),
            columns: Vec::new(),
            etag: String::new(),
        }
    }
}

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

/// A host's columns for a source, as the JSON of an [`InferredDescriptor`].
///
/// # Errors
///
/// `api/invalid-argument`, the `serde_json` error as its cause, naming
/// `descriptorJson` — the argument both hosts take it as.
// `result_large_err`: a host refuses the call with this `Failure` whole, once,
// on the path that ends it — `fossil-exec`'s crate root says the same.
#[allow(clippy::result_large_err)]
pub fn columns_from_json(json: &str) -> Result<InferredDescriptor, Failure> {
    serde_json::from_str(json).map_err(|e| {
        Failure::new(Problem::InvalidArgument {
            argument: "descriptorJson".to_string(),
            expected: "an InferredDescriptor as JSON".to_string(),
        })
        .caused_by(e)
    })
}

/// Why a host could not describe a source, as the JSON of the `Problem` its
/// profile answered (`{ code, data, help?, … }`, as `@fossil-lang/types`
/// carries it) — a [`Described::Refused`].
///
/// `title`, `detail` and `cause` are not read: the first two are rendered
/// again from the code and its data, and a diagnostic has no place for the
/// third.
///
/// # Errors
///
/// `api/invalid-argument`, the `serde_json` error as its cause, naming
/// `problemJson`, when it is not a `Problem` this build of fossil knows.
#[allow(clippy::result_large_err)] // as `columns_from_json`.
pub fn refusal_from_json(json: &str) -> Result<Described, Failure> {
    #[derive(serde::Deserialize)]
    struct Refusal {
        #[serde(flatten)]
        problem: Problem,
        #[serde(default)]
        help: Option<String>,
    }
    let Refusal { problem, help } = serde_json::from_str(json).map_err(|e| {
        Failure::new(Problem::InvalidArgument {
            argument: "problemJson".to_string(),
            expected: "a Problem as JSON".to_string(),
        })
        .caused_by(e)
    })?;
    Ok(Described::Refused { problem, help })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

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
    fn inferred_descriptor_round_trips_through_serde() {
        let d = InferredDescriptor {
            key: "examples/users.csv".into(),
            columns: vec![
                InferredColumn {
                    name: "id".into(),
                    primitive: Primitive::Integer,
                },
                InferredColumn {
                    name: "name".into(),
                    primitive: Primitive::String,
                },
            ],
            etag: "abc123".into(),
        };
        let j = serde_json::to_string(&d).expect("serialise");
        let d2: InferredDescriptor = serde_json::from_str(&j).expect("deserialise");
        assert_eq!(d, d2);
    }

    /// The host's wire format is the lattice itself. A name outside it is
    /// rejected here — where the JSON arrives and the offending value is still
    /// in hand — instead of being coerced to `String` and reported by the
    /// checker as a diagnostic about a column.
    #[test]
    fn a_primitive_outside_the_lattice_is_a_deserialisation_error() {
        let json = r#"{"key":"examples/users.csv",
                       "columns":[{"name":"id","primitive":"decimal"}],
                       "etag":""}"#;
        let err = serde_json::from_str::<InferredDescriptor>(json)
            .expect_err("`decimal` is not in the lattice");
        assert!(
            err.to_string().contains("decimal"),
            "the error names the value the host sent: {err}"
        );
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
