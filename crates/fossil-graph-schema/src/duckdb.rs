//! `DuckDB`, at the profiling border.
//!
//! A source is profiled by asking `DuckDB` to `DESCRIBE` it, and the answer types
//! each column in `DuckDB`'s words. [`Duckdb`] wraps one of those words and
//! `TryFrom<&Duckdb>` reads it into the lattice, as [`Xsd`](crate::Xsd) does
//! for a datatype IRI: by the type's name, whole, and never by a prefix or by
//! the order of a table. The read is a `TryFrom` because the lattice is smaller
//! than `DuckDB`: an `INTERVAL`, a `BLOB` or a `STRUCT` has no primitive, and the
//! caller decides what a column of one is.

use serde::{Deserialize, Serialize};

use crate::{Lang, OutsideLattice, Precision, Primitive};

/// A `DuckDB` column type, exactly as `DESCRIBE` prints it: `BIGINT`,
/// `DECIMAL(18,3)`, `TIMESTAMP WITH TIME ZONE`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Duckdb(String);

impl Duckdb {
    /// The column type `name` spells.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The type as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Duckdb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The type a column of each primitive has when `DuckDB` reads a graph fossil
/// wrote: every primitive is stored as one `GraphAr` type whatever the source
/// handed the run (`fossil_sinks::graphar`), so a host can say a graph's column
/// types before the run. It is not the inverse of the read below: an hour and an
/// identifier are written as text, and a tagged text as `GraphAr`'s user-defined
/// `langString`. It is one value's type: a property that holds a set is a list
/// of it, `T[]`.
impl From<Primitive> for Duckdb {
    fn from(primitive: Primitive) -> Self {
        Self::new(match primitive {
            Primitive::Integer => "BIGINT",
            Primitive::Float => "DOUBLE",
            Primitive::Bool => "BOOLEAN",
            Primitive::Time(Precision::Year) => "INTEGER",
            Primitive::Time(Precision::Date) => "DATE",
            Primitive::Time(Precision::DateTime) => "TIMESTAMP",
            Primitive::String(Lang::Tagged) => "STRUCT(\"value\" VARCHAR, lang VARCHAR)",
            Primitive::String(Lang::Untagged)
            | Primitive::Time(Precision::Hour)
            | Primitive::Uri => "VARCHAR",
        })
    }
}

/// The only `DuckDB` → [`Primitive`] table in the tree. It reads the type by its
/// name, case-blind and without its parameters (`DECIMAL(18,3)` is a
/// `DECIMAL`), and the names are `DuckDB`'s `LogicalTypeId` spellings, the ones
/// `DESCRIBE` prints.
///
/// The unsigned integers are outside, as they were when this was a table of
/// patterns: the writer refuses a `UBIGINT` above `i64::MAX`, and nothing has
/// asked for the rest. `DECIMAL` is a `Float` because the lattice has no
/// decimal yet.
impl TryFrom<&Duckdb> for Primitive {
    type Error = OutsideLattice;

    fn try_from(duckdb: &Duckdb) -> Result<Self, OutsideLattice> {
        let name = duckdb.0.split('(').next().unwrap_or_default().trim();
        Ok(match name.to_ascii_uppercase().as_str() {
            "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "HUGEINT" => Self::Integer,
            "FLOAT" | "DOUBLE" | "DECIMAL" => Self::Float,
            "BOOLEAN" => Self::Bool,
            "DATE" => Self::DATE,
            "TIMESTAMP"
            | "TIMESTAMP_S"
            | "TIMESTAMP_MS"
            | "TIMESTAMP_NS"
            | "TIMESTAMP WITH TIME ZONE" => Self::DATE_TIME,
            "TIME" | "TIME_NS" | "TIME WITH TIME ZONE" => Self::HOUR,
            "VARCHAR" => Self::STRING,
            _ => return Err(OutsideLattice),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(name: &str) -> Option<Primitive> {
        Primitive::try_from(&Duckdb::new(name)).ok()
    }

    #[test]
    fn describe_types_read_into_the_lattice() {
        assert_eq!(read("BIGINT"), Some(Primitive::Integer));
        assert_eq!(read("bigint"), Some(Primitive::Integer));
        assert_eq!(read("DECIMAL(18,3)"), Some(Primitive::Float));
        assert_eq!(read("VARCHAR"), Some(Primitive::STRING));
        assert_eq!(read("DATE"), Some(Primitive::DATE));
    }

    /// Every one of `DuckDB`'s instants is a `Time<DateTime>`, and the order they
    /// are listed in decides nothing: `TIMESTAMP WITH TIME ZONE` is what
    /// `read_csv_auto` infers for an ISO-8601 string with an offset, how
    /// LDBC-SNB dates every row it ships.
    #[test]
    fn an_instant_is_never_an_hour() {
        for name in [
            "TIMESTAMP",
            "TIMESTAMP_S",
            "TIMESTAMP_MS",
            "TIMESTAMP_NS",
            "TIMESTAMP WITH TIME ZONE",
        ] {
            assert_eq!(read(name), Some(Primitive::DATE_TIME), "{name}");
        }
        for name in ["TIME", "TIME_NS", "TIME WITH TIME ZONE"] {
            assert_eq!(read(name), Some(Primitive::HOUR), "{name}");
        }
    }

    /// Outside the lattice is an error, never a silent `String`: the caller
    /// says what a column of one is.
    #[test]
    fn a_type_outside_the_lattice_is_refused() {
        for name in [
            "INTERVAL",
            "BLOB",
            "UUID",
            "UBIGINT",
            "STRUCT(a INTEGER)",
            "INTEGER[]",
            "",
        ] {
            assert_eq!(read(name), None, "{name}");
        }
    }
}
