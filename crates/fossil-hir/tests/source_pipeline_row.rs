//! The row algebra of a source pipeline (ADR-0054 §4): what `where`, `select`
//! and `join` do to the type of a row.
//!
//! Only this crate can test it, because it needs a real descriptor behind the
//! base binding — a pipeline over a source that declares no columns has no row
//! to transform, and the interesting refusals (a name that would appear twice, a
//! key typed two ways) are exactly the ones an untyped source cannot raise.

use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::Primitive;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::def_map;
use fossil_hir::ty::TyKind;
use std::sync::Arc;

fn descriptor(uri: &str, columns: &[(&str, Primitive)]) -> InferredDescriptor {
    InferredDescriptor {
        uri: uri.into(),
        columns: columns
            .iter()
            .map(|(n, p)| InferredColumn {
                name: (*n).into(),
                primitive: *p,
            })
            .collect(),
        freshness_token: "t1".into(),
    }
}

/// `pedidos` and `personas`, joinable on `persona_id` and sharing nothing else.
fn db_with(program: &str, descriptors: Vec<InferredDescriptor>) -> (FossilDb, SourceFile) {
    let system = NativeSystem::default();
    let cache = system
        .descriptors()
        .expect("the native host keeps a descriptor table");
    for d in descriptors {
        cache.insert(d);
    }
    let db = FossilDb::new(Arc::new(system) as Arc<dyn System>);
    let file = SourceFile::new(&db, program.to_string(), "/w/mapping.fossil".to_string());
    (db, file)
}

fn two_sources() -> Vec<InferredDescriptor> {
    vec![
        descriptor(
            "o.csv",
            &[
                ("id", Primitive::Integer),
                ("persona_id", Primitive::Integer),
                ("total", Primitive::Integer),
            ],
        ),
        descriptor(
            "p.csv",
            &[
                ("persona_id", Primitive::Integer),
                ("nombre", Primitive::String),
            ],
        ),
    ]
}

fn program(pipe: &str) -> String {
    format!(
        "prefix ex: <https://example.org/>\n\
         pedidos := io.csv(\"o.csv\")\n\
         personas := io.csv(\"p.csv\")\n\
         {pipe}\n\
         Venta : ex:Person from ventas\n    \
         ex:name = .id\n"
    )
}

/// Column names of the row the mapping sees, or the diagnostics that stopped it.
fn row_of(db: &FossilDb, file: SourceFile) -> Result<Vec<String>, Vec<String>> {
    let mappings = def_map(db, file).mappings(db).clone();
    let mapping = *mappings.first().expect("one mapping");
    match typecheck_mapping(db, mapping) {
        Ok(out) => {
            let Some(row) = out.source_row(db) else {
                return Ok(Vec::new());
            };
            let TyKind::Record(rec) = row.kind(db) else {
                panic!("a source row is a Record");
            };
            Ok(rec.fields(db).iter().map(|f| f.name.to_string()).collect())
        }
        Err(_) => Err(typecheck_mapping::accumulated::<Diagnostic>(db, mapping)
            .iter()
            .map(|d| d.message.clone())
            .collect()),
    }
}

/// `where` keeps the row, `join` unions it with the key appearing ONCE, and
/// `select` restricts it — in that order, through one pipeline.
#[test]
fn the_three_verbs_compose_and_the_join_key_appears_once() {
    let (db, file) = db_with(
        &program("ventas := pedidos |> join(personas, on = .persona_id) |> where(.total >= 100)"),
        two_sources(),
    );
    assert_eq!(
        row_of(&db, file).expect("types"),
        ["id", "persona_id", "total", "nombre"]
    );

    let (db, file) = db_with(
        &program(
            "unidas := pedidos |> join(personas, on = .persona_id)\n\
             ventas := unidas |> select(.id, .nombre)",
        ),
        two_sources(),
    );
    assert_eq!(row_of(&db, file).expect("types"), ["id", "nombre"]);
}

/// A name that would appear twice is refused, and the message says which.
///
/// This is the half of ADR-0054 §4 that is easy to get wrong in the other
/// direction: shadowing one side silently would give the mapping a column whose
/// meaning depends on which file the reader happens to know.
#[test]
fn a_join_that_would_duplicate_a_column_is_an_error() {
    let (db, file) = db_with(
        &program("ventas := pedidos |> join(personas, on = .persona_id)"),
        vec![
            descriptor(
                "o.csv",
                &[
                    ("id", Primitive::Integer),
                    ("persona_id", Primitive::Integer),
                    ("nombre", Primitive::String),
                ],
            ),
            descriptor(
                "p.csv",
                &[
                    ("persona_id", Primitive::Integer),
                    ("nombre", Primitive::String),
                ],
            ),
        ],
    );
    let errs = row_of(&db, file).expect_err("a duplicated column must refuse");
    assert!(
        errs.iter().any(|m| m.contains("`nombre`")),
        "the diagnostic must name the colliding column, got {errs:?}"
    );
}

/// The key must be the same type on both sides — no implicit coercion, the same
/// rule the ternary's branches follow (ADR-0050 §1, ADR-0054 §5).
#[test]
fn a_join_key_typed_two_ways_is_an_error() {
    let (db, file) = db_with(
        &program("ventas := pedidos |> join(personas, on = .persona_id)"),
        vec![
            descriptor(
                "o.csv",
                &[
                    ("id", Primitive::Integer),
                    ("persona_id", Primitive::Integer),
                ],
            ),
            descriptor(
                "p.csv",
                &[
                    ("persona_id", Primitive::String),
                    ("nombre", Primitive::String),
                ],
            ),
        ],
    );
    let errs = row_of(&db, file).expect_err("a key typed two ways must refuse");
    assert!(
        errs.iter()
            .any(|m| m.contains("Integer") && m.contains("String")),
        "the diagnostic must show both types, got {errs:?}"
    );
}

/// A column the row does not have, named by `select` or read by `where`, is a
/// compile error rather than a corpus with a column missing or a filter that
/// kept everything.
#[test]
fn a_column_the_row_does_not_have_is_an_error() {
    let (db, file) = db_with(
        &program("ventas := pedidos |> select(.id, .apellido)"),
        two_sources(),
    );
    let errs = row_of(&db, file).expect_err("an unknown column must refuse");
    assert!(errs.iter().any(|m| m.contains("apellido")), "got {errs:?}");

    let (db, file) = db_with(
        &program("ventas := pedidos |> where(.apellido >= 18)"),
        two_sources(),
    );
    let errs = row_of(&db, file).expect_err("an unknown column in a predicate must refuse");
    assert!(errs.iter().any(|m| m.contains("apellido")), "got {errs:?}");
}
