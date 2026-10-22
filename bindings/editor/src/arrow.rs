//! An answer's fields as a JS engine hands them over: apache-arrow's `Field`
//! and `DataType`, read into arrow-rs's. apache-arrow numbers a type as Arrow's
//! IPC schema does (`Schema.fbs`'s `Type`, and `-1` for a dictionary) and keeps
//! each parameter under the name `Schema.fbs` gives it, so this is that
//! format's reading, field by field, and not a table of any engine's.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields, IntervalUnit, TimeUnit, UnionFields, UnionMode};
use serde::Deserialize;

/// apache-arrow's `Field`: what `@fossil-lang/wasm` sends of one.
// `pub(crate)` plus an allow, as in `wasm_system.rs`: `unreachable_pub` and the
// `redundant_pub_crate` nursery lint each refuse the other's spelling.
#[derive(Debug, Deserialize)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct JsField {
    name: String,
    nullable: bool,
    #[serde(rename = "type")]
    data_type: JsType,
}

/// apache-arrow's `DataType`: its `typeId` and the parameters its type has.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsType {
    type_id: i32,
    bit_width: Option<u16>,
    is_signed: Option<bool>,
    precision: Option<u8>,
    scale: Option<i8>,
    unit: Option<u8>,
    timezone: Option<String>,
    byte_width: Option<i32>,
    list_size: Option<i32>,
    keys_sorted: Option<bool>,
    mode: Option<u8>,
    type_ids: Option<Vec<i8>>,
    children: Option<Vec<JsField>>,
    dictionary: Option<Box<Self>>,
    indices: Option<Box<Self>>,
}

impl JsField {
    /// The field, or what about it is not an Arrow type.
    pub(crate) fn read(self) -> Result<Field, String> {
        Ok(Field::new(self.name, self.data_type.read()?, self.nullable))
    }
}

fn time_unit(unit: Option<u8>) -> Result<TimeUnit, String> {
    Ok(match unit {
        Some(0) => TimeUnit::Second,
        Some(1) => TimeUnit::Millisecond,
        Some(2) => TimeUnit::Microsecond,
        Some(3) => TimeUnit::Nanosecond,
        other => return Err(format!("time unit {other:?}")),
    })
}

impl JsType {
    fn read(self) -> Result<DataType, String> {
        let Self {
            type_id,
            bit_width,
            is_signed,
            precision,
            scale,
            unit,
            timezone,
            byte_width,
            list_size,
            keys_sorted,
            mode,
            type_ids,
            children,
            dictionary,
            indices,
        } = self;
        let mut children = children.unwrap_or_default().into_iter().map(JsField::read);
        let mut child = || -> Result<Arc<Field>, String> {
            Ok(Arc::new(children.next().ok_or("a list with no child")??))
        };
        Ok(match type_id {
            1 => DataType::Null,
            2 => match (is_signed, bit_width) {
                (Some(true), Some(8)) => DataType::Int8,
                (Some(true), Some(16)) => DataType::Int16,
                (Some(true), Some(32)) => DataType::Int32,
                (Some(true), Some(64)) => DataType::Int64,
                (Some(false), Some(8)) => DataType::UInt8,
                (Some(false), Some(16)) => DataType::UInt16,
                (Some(false), Some(32)) => DataType::UInt32,
                (Some(false), Some(64)) => DataType::UInt64,
                other => return Err(format!("an integer of {other:?}")),
            },
            3 => match precision {
                Some(0) => DataType::Float16,
                Some(1) => DataType::Float32,
                Some(2) => DataType::Float64,
                other => return Err(format!("a float of precision {other:?}")),
            },
            4 => DataType::Binary,
            5 => DataType::Utf8,
            6 => DataType::Boolean,
            7 => {
                let (p, s) = (precision.unwrap_or(0), scale.unwrap_or(0));
                match bit_width.unwrap_or(128) {
                    32 => DataType::Decimal32(p, s),
                    64 => DataType::Decimal64(p, s),
                    128 => DataType::Decimal128(p, s),
                    256 => DataType::Decimal256(p, s),
                    other => return Err(format!("a decimal of {other} bits")),
                }
            }
            8 => match unit {
                Some(0) => DataType::Date32,
                Some(1) => DataType::Date64,
                other => return Err(format!("a date of unit {other:?}")),
            },
            9 => match bit_width {
                Some(32) => DataType::Time32(time_unit(unit)?),
                _ => DataType::Time64(time_unit(unit)?),
            },
            10 => DataType::Timestamp(time_unit(unit)?, timezone.map(Arc::from)),
            11 => DataType::Interval(match unit {
                Some(0) => IntervalUnit::YearMonth,
                Some(1) => IntervalUnit::DayTime,
                Some(2) => IntervalUnit::MonthDayNano,
                other => return Err(format!("an interval of unit {other:?}")),
            }),
            12 => DataType::List(child()?),
            13 => DataType::Struct(children.collect::<Result<Fields, _>>()?),
            14 => {
                let fields: Vec<Field> = children.collect::<Result<_, _>>()?;
                let ids = type_ids.unwrap_or_else(|| (0..).take(fields.len()).collect());
                DataType::Union(
                    UnionFields::try_new(ids, fields).map_err(|e| e.to_string())?,
                    if mode == Some(1) {
                        UnionMode::Dense
                    } else {
                        UnionMode::Sparse
                    },
                )
            }
            15 => DataType::FixedSizeBinary(byte_width.unwrap_or(0)),
            16 => DataType::FixedSizeList(child()?, list_size.unwrap_or(0)),
            17 => DataType::Map(child()?, keys_sorted.unwrap_or(false)),
            18 => DataType::Duration(time_unit(unit)?),
            19 => DataType::LargeBinary,
            20 => DataType::LargeUtf8,
            21 => DataType::LargeList(child()?),
            23 => DataType::BinaryView,
            24 => DataType::Utf8View,
            -1 => match (indices, dictionary) {
                (Some(indices), Some(values)) => {
                    DataType::Dictionary(Box::new(indices.read()?), Box::new(values.read()?))
                }
                _ => return Err("a dictionary with no indices or values".into()),
            },
            other => return Err(format!("type id {other}")),
        })
    }
}
