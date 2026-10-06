//! `fossil.json` — the one document a `fossil/1` corpus carries.
//!
//! A corpus is this file and Parquet: one table per vertex type under
//! `vertex/`, one per relation under `edge/`, and `fossil.json` at the root,
//! written **last** — its presence is the commit. The structs below are the
//! format. `fossil-df` builds a [`Manifest`] and serialises it; nothing in the
//! workspace parses one back, and a reader in another language checks itself
//! against `fossil.schema.json` beside this crate, which `tests/schema.rs`
//! holds against these structs.
//!
//! The vocabulary is SQL/PGQ's rather than `GraphAr`'s: a vertex table and an
//! edge table, a `key`, a `source` and a `destination` that each name the key
//! column and the vertex table it `references`. `/docs/design/corpus` is the
//! argument.
//!
//! # Versions
//!
//! Inside `fossil/1` only optional fields a reader may ignore are added.
//! Changing what a fixed column or a byte means is `fossil/2`. A reader refuses
//! a `format` it does not know before it reads a byte of Parquet, and ignores
//! any field it does not know.

use arrow_schema::DataType;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::generated::ColumnRole;

/// The format a corpus is written in, and what [`Manifest::format`] says.
///
/// **Every writer reads this; every assertion spells the literal.** A site
/// that *produces* a manifest routes through the constant, or changing it here
/// changes nothing the writer emits; a site that *checks* what came out spells
/// `fossil/1` by hand, because comparing against this constant passes whatever
/// it says. `crates/xtask/tests/version_string_is_one_constant.rs` holds the
/// first half.
pub const FOSSIL_FORMAT: &str = "fossil/1";

/// R2RML's term type of an IRI, as [`Property::term_type`] holds it.
pub const RR_IRI: &str = "http://www.w3.org/ns/r2rml#IRI";

/// R2RML's term type of a literal, as [`Property::term_type`] holds it.
pub const RR_LITERAL: &str = "http://www.w3.org/ns/r2rml#Literal";

/// The manifest's path under a corpus's root.
pub const MANIFEST_FILE: &str = "fossil.json";

/// Rows per Parquet row group, in every table of a corpus.
///
/// `DuckDB`'s own default, set explicitly because parquet-rs defaults to
/// 1,048,576 — which at a million vertices is one row group and no pruning by
/// the statistics a `dense_id` range reads. A reader prunes on the footer's
/// statistics; nothing reads this number back.
pub const ROW_GROUP_ROWS: usize = 122_880;

/// The whole of `fossil.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Manifest {
    /// Always [`FOSSIL_FORMAT`] for a corpus this writer produces.
    pub format: String,
    /// One per vertex type, in the order the compiled schema lists them.
    pub vertex_tables: Vec<VertexTable>,
    /// One per relation, in the order the compiled schema lists them.
    pub edge_tables: Vec<EdgeTable>,
}

/// One vertex type's table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VertexTable {
    /// The type's label: the name of the table and its PGQ label.
    pub name: String,
    /// The type's IRI, when the program's shape document gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// The table's Parquet file, relative to the corpus root.
    pub path: String,
    /// The key column: `dense_id`, global and gapless across every vertex table,
    /// and one contiguous range of it per table, in manifest order.
    pub key: String,
    /// The identity column: `subject`, unique within the table.
    pub identity: String,
    /// Rows in the file.
    pub record_count: u64,
    /// Every column of the file, in file order.
    pub properties: Vec<Property>,
}

/// One relation's table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EdgeTable {
    /// `<Src>_<label>_<Dst>`, unique: the label repeats across relations.
    pub name: String,
    /// The relation's label, e.g. `placedBy`.
    pub label: String,
    /// The relation's IRI, when the program's shape document gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// The table's Parquet file, relative to the corpus root.
    pub path: String,
    /// The column holding the source vertex's `dense_id`, and its table.
    pub source: Endpoint,
    /// The column holding the destination vertex's `dense_id`, and its table.
    pub destination: Endpoint,
    /// Rows in the file.
    pub record_count: u64,
    /// Every column of the file, in file order.
    pub properties: Vec<Property>,
}

/// One end of a relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Endpoint {
    /// The edge table's column.
    pub key: String,
    /// The vertex table whose `key` it holds.
    pub references: String,
}

/// One column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Property {
    /// The column name, as in the Parquet schema.
    pub name: String,
    /// Its type, in the spelling [`data_type_name`] writes.
    #[serde(rename = "type")]
    pub data_type: String,
    /// The predicate IRI the program mapped it from, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// The RDF term type the shape declares for the values, as R2RML's IRI —
    /// [`RR_IRI`] or [`RR_LITERAL`] — which a mapping states as `rr:termType`.
    /// Absent where no shape declares one, and a reader then takes R2RML's
    /// default: a literal of the column's natural datatype (R2RML §10.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term_type: Option<String>,
    /// The literal datatype the shape declares, as the IRI it wrote — a
    /// mapping's `rr:datatype`. Absent where it declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datatype: Option<String>,
    /// Whether a row may hold no value. Absent is `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nullable: bool,
    /// What the column IS, for a column the writer emits — `corpus.bnf`'s role, so
    /// a reader names the roles it means rather than the names it remembers.
    /// Absent on a column of the program's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<ColumnRole>,
}

impl Manifest {
    /// The document as it is written: pretty JSON with a trailing newline.
    ///
    /// # Errors
    /// Never for these types; `serde_json`'s signature is fallible.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self).map(|mut s| {
            s.push('\n');
            s
        })
    }

    /// The JSON Schema of the document — what `fossil.schema.json` holds.
    ///
    /// # Panics
    /// Never: a derived schema always serialises.
    #[must_use]
    pub fn json_schema() -> String {
        let schema = schemars::schema_for!(Self);
        let mut text = serde_json::to_string_pretty(&schema).expect("a derived schema serialises");
        text.push('\n');
        text
    }
}

/// A vertex table's path: `vertex/<name>.parquet`.
#[must_use]
pub fn vertex_path(name: &str) -> String {
    format!("vertex/{name}.parquet")
}

/// A relation's table name: `<Src>_<label>_<Dst>`.
#[must_use]
pub fn edge_table_name(source: &str, label: &str, destination: &str) -> String {
    format!("{source}_{label}_{destination}")
}

/// An edge table's path: `edge/<name>.parquet`.
#[must_use]
pub fn edge_path(name: &str) -> String {
    format!("edge/{name}.parquet")
}

/// The manifest's spelling of an Arrow type.
///
/// `float` and `double` are `corpus.bnf`'s spellings; the integer widths are
/// exact. A list is `list<…>` of its item. `uint32` is the key's and the
/// endpoints' type.
#[must_use]
pub fn data_type_name(dt: &DataType) -> String {
    match dt {
        DataType::Boolean => "bool".to_string(),
        DataType::Int8 => "int8".to_string(),
        DataType::Int16 => "int16".to_string(),
        DataType::Int32 => "int32".to_string(),
        DataType::Int64 => "int64".to_string(),
        DataType::UInt8 => "uint8".to_string(),
        DataType::UInt16 => "uint16".to_string(),
        DataType::UInt32 => "uint32".to_string(),
        DataType::UInt64 => "uint64".to_string(),
        DataType::Float16 | DataType::Float32 => "float".to_string(),
        DataType::Float64 => "double".to_string(),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "string".to_string(),
        DataType::Date32 | DataType::Date64 => "date".to_string(),
        DataType::Timestamp(_, _) => "timestamp".to_string(),
        DataType::Time32(_) | DataType::Time64(_) => "time".to_string(),
        DataType::Decimal32(_, _)
        | DataType::Decimal64(_, _)
        | DataType::Decimal128(_, _)
        | DataType::Decimal256(_, _) => "decimal".to_string(),
        DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _) => {
            format!("list<{}>", data_type_name(item.data_type()))
        }
        _ => "binary".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::Field;
    use std::sync::Arc;

    fn column(name: &str, data_type: &str, nullable: bool) -> Property {
        Property {
            name: name.to_string(),
            data_type: data_type.to_string(),
            iri: None,
            term_type: None,
            datatype: None,
            nullable,
            role: None,
        }
    }

    fn manifest() -> Manifest {
        Manifest {
            format: FOSSIL_FORMAT.to_string(),
            vertex_tables: vec![VertexTable {
                name: "Person".to_string(),
                iri: Some("https://example.org/Person".to_string()),
                path: vertex_path("Person"),
                key: "dense_id".to_string(),
                identity: "subject".to_string(),
                record_count: 3,
                properties: vec![
                    Property {
                        role: Some(ColumnRole::Address),
                        ..column("dense_id", "uint32", false)
                    },
                    {
                        let mut name = column("name", "string", true);
                        name.iri = Some("https://example.org/name".to_string());
                        name
                    },
                ],
            }],
            edge_tables: vec![EdgeTable {
                name: edge_table_name("Person", "knows", "Person"),
                label: "knows".to_string(),
                iri: None,
                path: edge_path("Person_knows_Person"),
                source: Endpoint {
                    key: "src".to_string(),
                    references: "Person".to_string(),
                },
                destination: Endpoint {
                    key: "dst".to_string(),
                    references: "Person".to_string(),
                },
                record_count: 2,
                properties: vec![
                    column("src", "uint32", false),
                    column("dst", "uint32", false),
                ],
            }],
        }
    }

    #[test]
    fn the_document_is_the_contract_shape() {
        let text = manifest().to_json().unwrap();
        assert!(text.contains("\"format\": \"fossil/1\""), "{text}");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let person = &json["vertex_tables"][0];
        assert_eq!(person["path"], "vertex/Person.parquet");
        assert_eq!(person["key"], "dense_id");
        assert_eq!(person["identity"], "subject");
        assert!(
            person.get("position").is_none(),
            "the corpus carries no picture"
        );
        assert_eq!(person["properties"][0]["type"], "uint32");
        assert!(
            person["properties"][0].get("nullable").is_none(),
            "a column that cannot be null omits the field"
        );
        assert_eq!(person["properties"][1]["nullable"], true);
        assert_eq!(person["properties"][0]["role"], "address");
        assert!(
            person["properties"][1].get("role").is_none(),
            "a program column carries no role"
        );
        let knows = &json["edge_tables"][0];
        assert_eq!(knows["name"], "Person_knows_Person");
        assert_eq!(knows["path"], "edge/Person_knows_Person.parquet");
        assert_eq!(knows["source"]["key"], "src");
        assert_eq!(knows["destination"]["references"], "Person");
        assert!(knows.get("iri").is_none(), "an absent IRI is absent");
    }

    #[test]
    fn the_document_round_trips() {
        let m = manifest();
        let back: Manifest = serde_json::from_str(&m.to_json().unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn a_reader_ignores_fields_it_does_not_know() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        json["extent"] = serde_json::json!([0, 0, 1, 1]);
        json["vertex_tables"][0]["extent"] = serde_json::json!([0, 0, 1, 1]);
        let back: Manifest = serde_json::from_value(json).unwrap();
        assert_eq!(back, manifest());
    }

    #[test]
    fn spellings_are_exact_widths_and_corpus_bnf_floats() {
        assert_eq!(data_type_name(&DataType::Int16), "int16");
        assert_eq!(data_type_name(&DataType::UInt32), "uint32");
        assert_eq!(data_type_name(&DataType::Float32), "float");
        assert_eq!(data_type_name(&DataType::Float64), "double");
        let list = DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)));
        assert_eq!(data_type_name(&list), "list<string>");
    }

    /// The role on the wire is `corpus.bnf`'s word for it, so a reader in another
    /// language matches the file it can read rather than a Rust variant name.
    #[test]
    fn a_role_is_spelled_as_corpus_bnf_writes_it() {
        for column in crate::generated::PAYLOAD_COLUMNS
            .iter()
            .chain(crate::generated::EDGE_COLUMNS)
        {
            let wire = serde_json::to_value(column.role).unwrap();
            assert_eq!(
                wire,
                serde_json::Value::String(format!("{:?}", column.role).to_lowercase()),
                "`{}`",
                column.name
            );
        }
    }

    #[test]
    fn every_generated_column_type_is_a_spelling_this_module_writes() {
        let spelled: Vec<String> = [DataType::UInt32, DataType::Utf8, DataType::Float32]
            .iter()
            .map(data_type_name)
            .collect();
        for column in crate::generated::PAYLOAD_COLUMNS
            .iter()
            .chain(crate::generated::EDGE_COLUMNS)
        {
            assert!(
                spelled.iter().any(|s| s == column.data_type),
                "`{}` is declared `{}`, which data_type_name never writes",
                column.name,
                column.data_type
            );
        }
    }
}
