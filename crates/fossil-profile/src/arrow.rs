//! What an Arrow column type means on the lattice.
//!
//! Every engine fossil reads a source with answers in Arrow: `DuckDB`, native
//! and in the browser, hands its results over as Arrow, and `DataFusion` is
//! built on it. So a source's columns are typed by the [`DataType`] its engine
//! answers `SELECT * … LIMIT 0` with, and [`primitive`] reads one into the
//! lattice, as [`Xsd`](fossil_graph_schema::Xsd) does a datatype IRI. No
//! engine's own spelling of a type is read anywhere.

use arrow_schema::DataType;
use fossil_graph_schema::{OutsideLattice, Primitive};

/// The only Arrow → [`Primitive`] table in the tree.
///
/// A decimal with no digit after the point holds integers, and is one — how
/// `DuckDB` answers a `HUGEINT`; any other is a `Float`, because the lattice has
/// no decimal yet. A `UInt64` is outside it: the writer refuses one above
/// `i64::MAX`. A type the
/// lattice has no primitive for — an interval, bytes, a list or a struct — is
/// [`OutsideLattice`], and the caller decides what a column of one is.
///
/// # Errors
///
/// [`OutsideLattice`] for a type no primitive holds.
pub const fn primitive(data_type: &DataType) -> Result<Primitive, OutsideLattice> {
    Ok(match data_type {
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::Decimal32(_, 0)
        | DataType::Decimal64(_, 0)
        | DataType::Decimal128(_, 0)
        | DataType::Decimal256(_, 0) => Primitive::Integer,
        DataType::Float16
        | DataType::Float32
        | DataType::Float64
        | DataType::Decimal32(..)
        | DataType::Decimal64(..)
        | DataType::Decimal128(..)
        | DataType::Decimal256(..) => Primitive::Float,
        DataType::Boolean => Primitive::Bool,
        DataType::Date32 | DataType::Date64 => Primitive::DATE,
        DataType::Timestamp(..) => Primitive::DATE_TIME,
        DataType::Time32(_) | DataType::Time64(_) => Primitive::HOUR,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Primitive::STRING,
        _ => return Err(OutsideLattice),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_schema::{Field, Fields, IntervalUnit, TimeUnit};

    use super::*;

    /// Every instant is a `Time<DateTime>`, whatever its unit or zone: a
    /// `TIMESTAMP WITH TIME ZONE` is what `read_csv_auto` infers for an
    /// ISO-8601 string with an offset, how LDBC-SNB dates every row it ships.
    #[test]
    fn an_instant_is_never_an_hour() {
        for unit in [
            TimeUnit::Second,
            TimeUnit::Millisecond,
            TimeUnit::Microsecond,
            TimeUnit::Nanosecond,
        ] {
            for zone in [None, Some(Arc::from("UTC"))] {
                let t = DataType::Timestamp(unit, zone);
                assert_eq!(primitive(&t), Ok(Primitive::DATE_TIME), "{t}");
            }
        }
        assert_eq!(
            primitive(&DataType::Time64(TimeUnit::Microsecond)),
            Ok(Primitive::HOUR)
        );
    }

    /// Outside the lattice is an error, never a silent `String`: the caller
    /// says what a column of one is.
    #[test]
    fn a_type_outside_the_lattice_is_refused() {
        for t in [
            DataType::Interval(IntervalUnit::MonthDayNano),
            DataType::Binary,
            DataType::UInt64,
            DataType::Null,
            DataType::new_list(DataType::Int32, true),
            DataType::Struct(Fields::from(vec![Field::new("a", DataType::Int32, true)])),
        ] {
            assert_eq!(primitive(&t), Err(OutsideLattice), "{t}");
        }
    }
}
