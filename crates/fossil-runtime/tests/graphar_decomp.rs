//! SC#4 — sink-path REAL files via a fixture `ShEx` (option b).
//!
//! NATIVE-ONLY (`fossil-runtime` carries a `wasm32` `compile_error!` tripwire).
//!
//! This is the test that proves SC#4 produces ACTUAL Parquet, not just a
//! type-check. It builds the worked-example two-shape `ShExDescriptor`
//! (`ex:Person { ex:name xsd:string ; ex:knows @ex:Person + }` +
//! `ex:Company { ex:legalName xsd:string }`), hand-builds the matching
//! `MirGraph`, runs the 05-08 `fossil_codegen::codegen_sql_with_descriptor`
//! chunked-COPY emission (the plain-Rust descriptor seam — ADR-0019), executes
//! the generated SQL on a native bundled-DuckDB 1.10502 connection into a temp
//! dir, and asserts on the real files:
//!
//! - the Person vertex emits >= 2 chunk Parquet files when `chunk_size <` its
//!   (collapsed) row count (SINK-03 larger-than-RAM proof);
//! - the decomposition yields 2 vertex tables (Person, Company) + 1 edge table
//!   (knows);
//! - duplicate person subjects COLLAPSE for the cardinality-1 `ex:name`
//!   (`DISTINCT ON (iri)`); the `knows` edges are NOT collapsed (`OneOrMore`);
//! - the vertex `id` column equals the IRI string VERBATIM (SINK-04);
//! - the emitted manifest is valid `GraphAr` v1.0.0 (`version: gar/v1`).
//!
//! Honesty (ADR-0018): the decomposition is driven by a host/test-supplied
//! fixture `ShEx` (option b); the end-to-end type-checker `Db`-wiring is Phase 6
//! and lights up the same seam with zero decomp changes.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::elidable_lifetime_names)]
// The doc strings + fixture comments embed ShEx / SQL fragments (spec syntax,
// not Rust items).
#![allow(clippy::literal_string_with_formatting_args, clippy::doc_markdown)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use duckdb::Connection;
use fossil_codegen::codegen_sql_with_descriptor;
use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{Expr, Op, SinkRef, SourceFormat};
use smol_str::SmolStr;

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

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
    fossil_base::FossilDb::new(system)
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

fn colref(source: &str, column: &str) -> Expr<'static> {
    Expr::ColRef {
        source: SmolStr::from(source),
        column: SmolStr::from(column),
    }
}

fn iri_subject() -> Expr<'static> {
    Expr::ColRef {
        source: SmolStr::default(),
        column: SmolStr::new_static("iri"),
    }
}

/// The mapping `MirGraph` for the SC#4 fixture: a `people` CSV source → an `iri`
/// Extend → three `TripleEmit`s (`ex:name` literal, `ex:knows` IRI-object,
/// `ex:legalName` literal — so the base relation exposes every column the two
/// shapes reference) → `Sink`. `uri` points at the on-disk fixture CSV.
fn fixture_ops<'db>(db: &'db dyn fossil_base::Db, csv_uri: &str) -> Vec<Op<'db>> {
    vec![
        Op::Source {
            uri: SmolStr::from(csv_uri),
            format: SourceFormat::Csv,
            row_type: string_record(db, &["id", "name", "friend", "legalname"]),
        },
        Op::Extend {
            input: 0,
            field: SmolStr::new_static("iri"),
            // The IRI template: http://example.org/<id>. Used verbatim as
            // vertex_id / src_id (SINK-04).
            expr: Expr::Concat(
                Box::new(Expr::LitString(SmolStr::new_static("http://example.org/"))),
                Box::new(colref("people", "id")),
            ),
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_subject(),
            predicate: SmolStr::new_static("http://example.org/name"),
            object: colref("people", "name"),
            graph: None,
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_subject(),
            predicate: SmolStr::new_static("http://example.org/knows"),
            // The friend column already holds the full object IRI verbatim.
            object: colref("people", "friend"),
            graph: None,
        },
        Op::TripleEmit {
            input: 1,
            subject: iri_subject(),
            predicate: SmolStr::new_static("http://example.org/legalName"),
            object: colref("people", "legalname"),
            graph: None,
        },
        Op::Sink {
            input: 4,
            sink: SinkRef::GraphAr,
        },
    ]
}

/// A salsa input carrying the fixture parameters into the tracked frame (the
/// CSV path + chunk_size; tracked structs like `MirGraph` may only be created
/// inside a tracked function).
#[salsa::input]
struct Fixture {
    #[returns(ref)]
    csv_uri: String,
    chunk_size: u64,
}

/// The generated SQL + manifest, returned out of the tracked frame.
#[salsa::tracked]
struct Generated<'db> {
    #[returns(ref)]
    sql: String,
    #[returns(ref)]
    manifest: String,
}

/// Build the descriptor-driven chunked-COPY SQL + manifest INSIDE a tracked
/// frame (so `MirGraph::new` / `Ty` interning are legal). The descriptor seam
/// itself is plain-Rust; the `row_count_for` oracle opens its own in-memory
/// DuckDB connection and runs a real `SELECT count(*)` over the inner SELECT so
/// the chunk count is exact (a plain-Rust emission concern — NOT a Salsa query,
/// ADR-0019).
#[salsa::tracked]
fn codegen_in_frame<'db>(db: &'db dyn fossil_base::Db, fx: Fixture) -> Generated<'db> {
    let csv_uri = fx.csv_uri(db).clone();
    let chunk_size = fx.chunk_size(db);
    let mir = MirGraph::new(db, fixture_ops(db, &csv_uri));
    // The oracle counts rows over the inner SELECT, which references the `people`
    // source view. Create that view on the oracle connection first so the count
    // resolves (the base relation reads `FROM people`).
    let count_conn = Connection::open_in_memory().expect("oracle DuckDB conn");
    count_conn
        .execute_batch(&format!(
            "CREATE VIEW people AS SELECT * FROM read_csv_auto('{csv_uri}', sample_size=-1);"
        ))
        .expect("create people view on oracle conn");
    let (sql, manifest) =
        codegen_sql_with_descriptor(db, mir, &two_shape_kind(), chunk_size, |inner| {
            let q = format!("SELECT count(*) FROM ({inner}) AS _c");
            count_conn
                .query_row(&q, [], |r| r.get::<_, i64>(0))
                .map(|n| u64::try_from(n.max(0)).unwrap_or(0))
                .ok()
        });
    Generated::new(db, sql, manifest)
}

/// Count `chunk*.parquet` files DuckDB wrote under `dir/<prefix>`.
fn chunk_files(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let table_dir = dir.join(prefix);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&table_dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("chunk") && n.ends_with(".parquet"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Read every `chunk*.parquet` for a table back as one relation, returning the
/// row count via `read_parquet('<dir>/<prefix>/chunk*.parquet')`.
fn read_back_count(conn: &Connection, dir: &Path, prefix: &str) -> i64 {
    let glob = dir.join(prefix).join("chunk*.parquet");
    let sql = format!(
        "SELECT count(*) FROM read_parquet('{}')",
        glob.to_string_lossy()
    );
    conn.query_row(&sql, [], |r| r.get::<_, i64>(0))
        .expect("read_parquet chunk glob")
}

#[test]
fn sc4_sink_path_writes_two_vertex_one_edge_parquet_with_multichunk_and_collapse() {
    // A unique temp dir for this test run (no tempfile dep — the runtime crate
    // is native-only and we clean up at the end).
    let dir = std::env::temp_dir().join(format!(
        "fossil_graphar_sc4_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir temp");

    // The fixture CSV: 3 distinct persons (p1..p3) with p1 DUPLICATED (same IRI,
    // same name) so the cardinality-1 ex:name collapse is observable. Each row
    // carries a `friend` (the knows-edge object IRI, OneOrMore — all kept) and
    // a `legalname` (Company). After collapse the Person vertex has 3 distinct
    // rows; with chunk_size = 2 that is ceil(3/2) = 2 chunk files (SINK-03).
    let csv = dir.join("people.csv");
    std::fs::write(
        &csv,
        "id,name,friend,legalname\n\
         p1,Alice,http://example.org/p2,Acme\n\
         p1,Alice,http://example.org/p3,Acme\n\
         p2,Bob,http://example.org/p3,Globex\n\
         p3,Carol,http://example.org/p1,Initech\n",
    )
    .expect("write csv");
    let csv_uri = csv.to_string_lossy().to_string();

    let conn = Connection::open_in_memory().expect("open in-memory DuckDB");

    // chunk_size = 2 < the 3 distinct Person rows → >= 2 Person chunk files.
    let chunk_size = 2;
    let db = db();
    let fx = Fixture::new(&db, csv_uri, chunk_size);
    let generated = codegen_in_frame(&db, fx);
    let sql = generated.sql(&db).clone();
    let manifest = generated.manifest(&db).clone();

    // The COPY targets in the generated SQL are RELATIVE
    // (`vertex/person/chunkN.parquet`). DuckDB COPY does not always create parent
    // directories, so pre-create the table dirs, then run with the temp dir as
    // CWD so the relative targets land under `dir`.
    for prefix in [
        "vertex/person",
        "vertex/company",
        "edge/person_knows_person",
    ] {
        std::fs::create_dir_all(dir.join(prefix)).expect("mkdir table dir");
    }
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&dir).expect("chdir temp");
    let exec = conn.execute_batch(&sql);
    std::env::set_current_dir(&prev_cwd).expect("restore cwd");
    exec.unwrap_or_else(|e| panic!("execute chunked COPY failed: {e}\n--- SQL ---\n{sql}"));

    // --- Assertions on the REAL files ---------------------------------------

    // 2 vertex tables (Person, Company) + 1 edge table (knows) — directories
    // exist with at least one chunk each.
    let person_chunks = chunk_files(&dir, "vertex/person");
    let company_chunks = chunk_files(&dir, "vertex/company");
    let knows_chunks = chunk_files(&dir, "edge/person_knows_person");

    assert!(
        person_chunks.len() >= 2,
        "SINK-03: Person vertex must write >= 2 chunk files at chunk_size={chunk_size} \
         (3 distinct persons), got {}: {person_chunks:?}",
        person_chunks.len()
    );
    assert!(
        !company_chunks.is_empty(),
        "Company vertex table must write >= 1 chunk file: {company_chunks:?}"
    );
    assert!(
        !knows_chunks.is_empty(),
        "knows edge table must write >= 1 chunk file: {knows_chunks:?}"
    );

    // Person rows collapse to 3 distinct (cardinality-1 ex:name → DISTINCT ON iri).
    let person_rows = read_back_count(&conn, &dir, "vertex/person");
    assert_eq!(
        person_rows, 3,
        "duplicate p1 subject must collapse to 3 distinct Person rows (SINK-05 cardinality-1)"
    );

    // The knows edges are NOT collapsed (OneOrMore → all 4 kept).
    let knows_rows = read_back_count(&conn, &dir, "edge/person_knows_person");
    assert_eq!(
        knows_rows, 4,
        "OneOrMore knows edges keep all rows (no collapse): one per CSV row"
    );

    // The vertex `id` column equals the IRI string VERBATIM (SINK-04 — no hash,
    // no sequential id). Read one id back and confirm it is an http://example.org/ IRI.
    let sample_id: String = conn
        .query_row(
            &format!(
                "SELECT id FROM read_parquet('{}') ORDER BY id LIMIT 1",
                dir.join("vertex/person")
                    .join("chunk*.parquet")
                    .to_string_lossy()
            ),
            [],
            |r| r.get(0),
        )
        .expect("read Person id");
    assert!(
        sample_id.starts_with("http://example.org/p"),
        "vertex id must be the IRI verbatim (SINK-04), got {sample_id:?}"
    );

    // The emitted manifest is valid GraphAr v1.0.0.
    assert!(
        manifest.contains("version: gar/v1"),
        "manifest:\n{manifest}"
    );
    assert!(manifest.contains("type: Person"), "manifest:\n{manifest}");
    assert!(manifest.contains("type: Company"), "manifest:\n{manifest}");
    assert!(
        manifest.contains("edge_type: knows"),
        "manifest:\n{manifest}"
    );
    assert!(
        !manifest.contains("graphar_version"),
        "must be the programmatic v1.0.0 manifest, not the Phase-1 template:\n{manifest}"
    );

    // Cleanup (best-effort).
    let _ = std::fs::remove_dir_all(&dir);
}
