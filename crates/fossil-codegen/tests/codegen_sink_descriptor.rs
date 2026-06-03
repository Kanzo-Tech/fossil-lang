//! Descriptor-driven Sink codegen snapshots (SINK-01 / SINK-03, plan 05-08).
//!
//! These tests drive [`fossil_codegen::codegen_sql_with_descriptor`] — the
//! PLAIN-RUST outer wrapper over the tracked `codegen_graph` (ADR-0019) — with a
//! hand-built `MirGraph` fixture + a fixture `ShExDescriptor` (SC#4 option (b)),
//! and snapshot the emitted per-vertex / per-edge chunked `COPY ... (FORMAT
//! PARQUET)` SQL.
//!
//! ## The multi-chunk proof (SINK-03 — larger-than-RAM)
//!
//! [`multi_chunk_emits_three_copy_statements`] sets `chunk_size = 1` over a
//! 3-row table (the `row_count_for` oracle returns `3`) and asserts the codegen
//! emits exactly THREE `COPY ... TO 'chunk{0,1,2}.parquet'` statements — proving
//! the range-chunked emission produces N > 1 chunk files, not a single COPY.
//!
//! ## The AcceptAll byte-identity guard
//!
//! [`accept_all_is_byte_identical_to_flat_copy`] proves the descriptor-less path
//! returns the tracked `codegen_graph` output verbatim (the walking-skeleton
//! invariant — `hello.fossil` unchanged).

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::elidable_lifetime_names)]
// The doc strings embed ShEx schema fragments (spec syntax, not Rust items).
#![allow(clippy::literal_string_with_formatting_args, clippy::doc_markdown)]

use std::sync::Arc;

use fossil_codegen::{codegen_graph, codegen_sql_with_descriptor};
use fossil_descriptors_output::{AcceptAllDescriptor, OutputDescriptorKind, ShExDescriptor};
use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{Expr, Op, SinkRef, SourceFormat};
use smol_str::SmolStr;

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
    fossil_base::FossilDb::new(system)
}

/// The SC#4 two-shape schema: `ex:Person { ex:name xsd:string ; ex:knows
/// @ex:Person + }` + `ex:Company { ex:legalName xsd:string }`.
const TWO_SHAPE_SCHEMA: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "EachOf",
          "expressions": [
            {
              "type": "TripleConstraint",
              "predicate": "http://example.org/name",
              "valueExpr": {
                "type": "NodeConstraint",
                "datatype": "http://www.w3.org/2001/XMLSchema#string"
              }
            },
            {
              "type": "TripleConstraint",
              "predicate": "http://example.org/knows",
              "valueExpr": "http://example.org/Person",
              "min": 1,
              "max": -1
            }
          ]
        }
      }
    },
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Company",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "TripleConstraint",
          "predicate": "http://example.org/legalName",
          "valueExpr": {
            "type": "NodeConstraint",
            "datatype": "http://www.w3.org/2001/XMLSchema#string"
          }
        }
      }
    }
  ]
}"#;

fn two_shape_kind() -> OutputDescriptorKind {
    let desc = ShExDescriptor::from_reader(TWO_SHAPE_SCHEMA.as_bytes()).expect("schema parses");
    assert!(
        desc.lowering_errors().is_empty(),
        "{:?}",
        desc.lowering_errors()
    );
    OutputDescriptorKind::ShEx(desc)
}

fn string_record<'db>(db: &'db dyn fossil_base::Db, names: &[&str]) -> Ty<'db> {
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    let fields: Vec<RecordField<'db>> = names
        .iter()
        .map(|n| RecordField {
            name: SmolStr::from(*n),
            ty: string_ty,
        })
        .collect();
    Ty::new(db, TyKind::Record(Record::new(db, fields)))
}

/// A `people` source → `iri` Extend → two `TripleEmit`s (`ex:name` literal,
/// `ex:knows` IRI-object) → `Sink`. Mirrors the SC#4 Person shape so the base
/// relation exposes the `iri`, `name`, and `knows` columns the decomposition
/// references.
fn person_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        Op::Source {
            uri: SmolStr::new_static("people.csv"),
            format: SourceFormat::Csv,
            row_type: string_record(db, &["id", "name", "friend"]),
            binding: SmolStr::new_static("people.csv"),
        },
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("http://example.org/"))),
                Box::new(Expr::ColRef {
                    source: SmolStr::new_static("people"),
                    column: SmolStr::new_static("id"),
                }),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("iri"),
            },
            predicate: SmolStr::new_static("http://example.org/name"),
            object: Expr::ColRef {
                source: SmolStr::new_static("people"),
                column: SmolStr::new_static("name"),
            },
            graph: None,
        },
        Op::TripleEmit {
            input: 1,
            subject: Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("iri"),
            },
            predicate: SmolStr::new_static("http://example.org/knows"),
            object: Expr::ColRef {
                source: SmolStr::new_static("people"),
                column: SmolStr::new_static("friend"),
            },
            graph: None,
        },
        Op::Sink {
            input: 3,
            sink: SinkRef::GraphAr,
        },
    ]
}

/// A minimal single-property mapping (no edges) used by the multi-chunk proof.
fn single_property_ops<'db>(db: &'db dyn fossil_base::Db) -> Vec<Op<'db>> {
    vec![
        Op::Source {
            uri: SmolStr::new_static("people.csv"),
            format: SourceFormat::Csv,
            row_type: string_record(db, &["id", "name"]),
            binding: SmolStr::new_static("people.csv"),
        },
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("http://example.org/"))),
                Box::new(Expr::ColRef {
                    source: SmolStr::new_static("people"),
                    column: SmolStr::new_static("id"),
                }),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("iri"),
            },
            predicate: SmolStr::new_static("http://example.org/name"),
            object: Expr::ColRef {
                source: SmolStr::new_static("people"),
                column: SmolStr::new_static("name"),
            },
            graph: None,
        },
        Op::Sink {
            input: 2,
            sink: SinkRef::GraphAr,
        },
    ]
}

#[salsa::tracked]
fn two_shape_sql(db: &dyn fossil_base::Db, marker: Marker) -> SqlOut<'_> {
    let _ = marker;
    let mir = MirGraph::new(db, person_ops(db));
    let (sql, manifest) = codegen_sql_with_descriptor(
        db,
        mir,
        &two_shape_kind(),
        // chunk_size large → one chunk per table (snapshot the COPY shape, not the chunk count).
        1024,
        |_inner| Some(1),
    );
    SqlOut::new(db, sql, manifest)
}

#[salsa::input]
struct Marker {
    n: u8,
}

#[salsa::tracked]
struct SqlOut<'db> {
    #[returns(ref)]
    sql: String,
    #[returns(ref)]
    manifest: String,
}

#[test]
fn two_shape_descriptor_emits_per_table_copy() {
    let db = db();
    let out = two_shape_sql(&db, Marker::new(&db, 0));
    let sql = out.sql(&db);

    // One COPY per vertex table (Person, Company) + one per edge table (knows).
    assert!(sql.contains("vertex/person/chunk0.parquet"), "{sql}");
    assert!(sql.contains("vertex/company/chunk0.parquet"), "{sql}");
    assert!(
        sql.contains("edge/person_knows_person/chunk0.parquet"),
        "{sql}"
    );
    assert!(sql.contains("FORMAT PARQUET"), "{sql}");
    // The Person vertex collapses duplicate subjects (ex:name Exact(1)).
    assert!(sql.contains("DISTINCT ON (iri)"), "{sql}");
    // The base relation projects iri + the predicate-local columns.
    assert!(sql.contains("AS iri"), "{sql}");
    assert!(sql.contains("AS name"), "{sql}");
    assert!(sql.contains("AS knows"), "{sql}");

    insta::assert_snapshot!("two_shape_sink_sql", sql);

    // The manifest is GraphAr v1.0.0 (programmatic path, not the Phase-1 template).
    let manifest = out.manifest(&db);
    assert!(manifest.contains("version: gar/v1"), "{manifest}");
    assert!(manifest.contains("type: Person"), "{manifest}");
    assert!(!manifest.contains("graphar_version"), "{manifest}");
}

/// SINK-03 multi-chunk proof: chunk_size = 1 over a 3-row vertex table emits
/// exactly THREE `COPY ... TO 'chunk{0,1,2}.parquet'` statements.
#[test]
fn multi_chunk_emits_three_copy_statements() {
    let db = db();
    let out = multi_chunk_sql(&db, Marker::new(&db, 1));
    let sql = out.sql(&db);

    // Exactly 3 chunk files, one COPY each.
    assert!(sql.contains("chunk0.parquet"), "{sql}");
    assert!(sql.contains("chunk1.parquet"), "{sql}");
    assert!(sql.contains("chunk2.parquet"), "{sql}");
    assert!(!sql.contains("chunk3.parquet"), "{sql}");

    let copy_count = sql.matches("COPY (").count();
    assert_eq!(
        copy_count, 3,
        "chunk_size=1 over 3 rows must emit 3 COPY statements (SINK-03): {sql}"
    );
    // Each COPY carries its range predicate.
    assert!(sql.contains("WHERE _rn BETWEEN 1 AND 1"), "{sql}");
    assert!(sql.contains("WHERE _rn BETWEEN 2 AND 2"), "{sql}");
    assert!(sql.contains("WHERE _rn BETWEEN 3 AND 3"), "{sql}");

    insta::assert_snapshot!("multi_chunk_three_copies_sql", sql);
}

#[salsa::tracked]
fn multi_chunk_sql(db: &dyn fossil_base::Db, marker: Marker) -> SqlOut<'_> {
    let _ = marker;
    let mir = MirGraph::new(db, single_property_ops(db));
    let (sql, manifest) =
        codegen_sql_with_descriptor(db, mir, &one_shape_kind(), 1, |_inner| Some(3));
    SqlOut::new(db, sql, manifest)
}

/// A single `ex:Person { ex:name xsd:string }` shape (one vertex, no edges).
fn one_shape_kind() -> OutputDescriptorKind {
    const SCHEMA: &str = r#"{
      "@context": "http://www.w3.org/ns/shex.jsonld",
      "type": "Schema",
      "shapes": [
        {
          "type": "ShapeDecl",
          "id": "http://example.org/Person",
          "shapeExpr": {
            "type": "Shape",
            "expression": {
              "type": "TripleConstraint",
              "predicate": "http://example.org/name",
              "valueExpr": {
                "type": "NodeConstraint",
                "datatype": "http://www.w3.org/2001/XMLSchema#string"
              }
            }
          }
        }
      ]
    }"#;
    let desc = ShExDescriptor::from_reader(SCHEMA.as_bytes()).expect("schema parses");
    OutputDescriptorKind::ShEx(desc)
}

/// The AcceptAll path returns the tracked `codegen_graph` output byte-identically
/// (walking-skeleton invariant).
#[test]
fn accept_all_is_byte_identical_to_flat_copy() {
    let db = db();
    let out = accept_all_sql(&db, Marker::new(&db, 2));
    let (with_desc_sql, with_desc_manifest) = (out.sql(&db), out.manifest(&db));
    let (flat_sql, flat_manifest) = (out.flat_sql(&db), out.flat_manifest(&db));
    assert_eq!(
        with_desc_sql, flat_sql,
        "AcceptAll SQL must be byte-identical"
    );
    assert_eq!(
        with_desc_manifest, flat_manifest,
        "AcceptAll manifest must be byte-identical"
    );
}

#[salsa::tracked]
struct AcceptAllOut<'db> {
    #[returns(ref)]
    sql: String,
    #[returns(ref)]
    manifest: String,
    #[returns(ref)]
    flat_sql: String,
    #[returns(ref)]
    flat_manifest: String,
}

#[salsa::tracked]
fn accept_all_sql(db: &dyn fossil_base::Db, marker: Marker) -> AcceptAllOut<'_> {
    let _ = marker;
    let mir = MirGraph::new(db, single_property_ops(db));
    let kind = OutputDescriptorKind::AcceptAll(AcceptAllDescriptor);
    let (sql, manifest) = codegen_sql_with_descriptor(db, mir, &kind, 1024, |_| None);
    let flat = codegen_graph(db, mir);
    AcceptAllOut::new(
        db,
        sql,
        manifest,
        flat.sql(db).clone(),
        flat.manifest_yaml(db).clone(),
    )
}
