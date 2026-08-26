//! The row algebra of a source pipeline: what `where`, `select`, `join`,
//! `distinct` and `union` do to the type of a row, and to the NAMES it is
//! addressed by.
//!
//! Only this crate can test it, because it needs a real descriptor behind the
//! base binding — a pipeline over a source that declares no columns has no row
//! to transform, and the interesting refusals (a column no side has, a binding
//! the pipeline never drew on) are exactly the ones an untyped source cannot
//! raise.
//!
//! A join keeps its two sides as two entries, each under its own binding, so a
//! joined row holds a shared column name TWICE — once per side — and `select`
//! is what narrows to the one you meant, naming a QUALIFIED column.
//!
//! # What this file does NOT prove
//!
//! **A join's condition is not TYPED.** `crate::infer::apply_source_op` runs
//! `check_refs` over it — which asks only that every column it names exists
//! under the binding that qualifies it — so
//! `pedidos.persona_id == personas.persona_id` with an `Integer` on one side
//! and a `String` on the other type-checks today. Asserting that here would pin
//! the gap as the contract, so it is written down instead.

use fossil_base::test_support::{db_with_document, register_inferred};
use fossil_base::{Diagnostic, FossilDb, SourceFile};
use fossil_graph_schema::Primitive;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::def_map;
use fossil_hir::ty::TyKind;

/// One shape, one un-narrowed predicate. The mapping below writes `name`, and
/// what it is checked against is not what this file is about — a narrowed
/// datatype here would make every row assertion depend on the body typing.
const DOCUMENT: &str = "\
shape http://example.org/Persona
prop http://example.org/name - 1 1
";

/// `pedidos` and `personas`, joinable on `persona_id` and sharing nothing else.
fn two_sources() -> Vec<(&'static str, &'static [(&'static str, Primitive)])> {
    vec![
        (
            "o.csv",
            &[
                ("id", Primitive::Integer),
                ("persona_id", Primitive::Integer),
                ("total", Primitive::Integer),
            ],
        ),
        (
            "p.csv",
            &[
                ("persona_id", Primitive::Integer),
                ("nombre", Primitive::String),
            ],
        ),
    ]
}

/// The whole program: the shape binding, the two sources, the pipeline under
/// test, and a mapping that reads it through `row.column`.
///
/// The identity is a required assignment on the body's first line, and naming a
/// shape document is mandatory — a program missing either does not reach the
/// row algebra at all, so neither is optional scaffolding here.
///
/// The body's reference is a PARAMETER because a `union`'s result answers to
/// the pipeline's name and to neither side's, so `pedidos.id` is not a
/// reference every pipeline here can be read through.
fn program(pipe: &str, row: &str, column: &str) -> String {
    format!(
        "type {{ Persona }} := io.shex(\"v.shex\")\n\
         pedidos := io.csv(\"o.csv\")\n\
         personas := io.csv(\"p.csv\")\n\
         {pipe}\n\
         Venta : Persona from ventas\n    \
         @subject = \"https://example.org/v/{{{row}.{column}}}\"\n    \
         name = {row}.{column}\n"
    )
}

fn db_reading(
    pipe: &str,
    row: &str,
    column: &str,
    descriptors: Vec<(&'static str, &'static [(&'static str, Primitive)])>,
) -> (FossilDb, SourceFile) {
    let (db, file) = db_with_document(&program(pipe, row, column), "v.shex", DOCUMENT);
    for (uri, columns) in descriptors {
        register_inferred(&db, uri, columns);
    }
    (db, file)
}

fn db_with(
    pipe: &str,
    descriptors: Vec<(&'static str, &'static [(&'static str, Primitive)])>,
) -> (FossilDb, SourceFile) {
    db_reading(pipe, "pedidos", "id", descriptors)
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

/// `where` keeps the row, `join` unions it with the key appearing ONCE PER
/// BINDING, and `select` restricts it — in that order, through one pipeline.
///
/// The `persona_id` twice is the assertion this test is here for. It is not a
/// duplicate the flattening failed to notice: the two sides are two entries of
/// the scope, `pedidos.persona_id` and `personas.persona_id` are two columns,
/// and flattening them into one list is what the mapping body never does. The
/// condition is an equality naming both sides, and a predicate relating two
/// columns does not merge them the way `USING (k)` would.
#[test]
fn the_three_verbs_compose_and_the_join_key_appears_once_per_binding() {
    let (db, file) = db_with(
        "ventas := pedidos.join(personas, on = pedidos.persona_id == personas.persona_id)\
         .where(pedidos.total >= 100)",
        two_sources(),
    );
    assert_eq!(
        row_of(&db, file).expect("types"),
        ["id", "persona_id", "total", "persona_id", "nombre"]
    );

    let (db, file) = db_with(
        "unidas := pedidos.join(personas, on = pedidos.persona_id == personas.persona_id)\n\
         ventas := unidas.select(pedidos.id, personas.nombre)",
        two_sources(),
    );
    assert_eq!(row_of(&db, file).expect("types"), ["id", "nombre"]);
}

/// `select` picks the SIDE as well as the column, so the two `persona_id`s of a
/// joined row are two things a projection can ask for apart.
///
/// `HirSourceOp::Select` carries the binding as well as the column for exactly
/// this: a bare name would be looked for in the rows in order and taken from
/// the first that had it, making `select(persona_id)` mean the left side's.
#[test]
fn select_after_a_join_names_the_side_it_keeps() {
    let joined = "unidas := pedidos.join(personas, on = pedidos.persona_id == personas.persona_id)";

    let (db, file) = db_with(
        &format!("{joined}\nventas := unidas.select(pedidos.id, personas.persona_id)"),
        two_sources(),
    );
    assert_eq!(row_of(&db, file).expect("types"), ["id", "persona_id"]);

    // The same column name off the LEFT side, and it is a different column.
    let (db, file) = db_with(
        &format!("{joined}\nventas := unidas.select(pedidos.id, pedidos.persona_id)"),
        two_sources(),
    );
    assert_eq!(row_of(&db, file).expect("types"), ["id", "persona_id"]);

    // A binding the pipeline never drew on is refused by name, and the message
    // says which rows it does have — the mistake is the QUALIFIER, and a
    // "column not found" would send the reader looking at the wrong half.
    let (db, file) = db_with(
        &format!("{joined}\nventas := unidas.select(clientes.nombre)"),
        two_sources(),
    );
    let errs = row_of(&db, file).expect_err("an unknown binding must refuse");
    assert!(
        errs.iter().any(|m| m.contains("clientes")),
        "the diagnostic must name the binding, got {errs:?}"
    );
}

/// A column the row does not have, named by `select` or read by `where`, is a
/// compile error rather than a corpus with a column missing or a filter that
/// kept everything.
#[test]
fn a_column_the_row_does_not_have_is_an_error() {
    let (db, file) = db_with(
        "ventas := pedidos.select(pedidos.id, pedidos.apellido)",
        two_sources(),
    );
    let errs = row_of(&db, file).expect_err("an unknown column must refuse");
    assert!(errs.iter().any(|m| m.contains("apellido")), "got {errs:?}");

    let (db, file) = db_with(
        "ventas := pedidos.where(pedidos.apellido >= 18)",
        two_sources(),
    );
    let errs = row_of(&db, file).expect_err("an unknown column in a predicate must refuse");
    assert!(errs.iter().any(|m| m.contains("apellido")), "got {errs:?}");
}

/// A `join` whose condition names a row the pipeline does not carry is refused,
/// and the message says which.
///
/// A shared column name means nothing, so what is left to get wrong is the
/// QUALIFIER: the question a condition must answer is not "does some side have
/// this column" but "does the row it names carry it".
#[test]
fn a_join_condition_naming_a_row_the_pipeline_lacks_is_an_error() {
    let (db, file) = db_with(
        "ventas := pedidos.join(personas, on = pedidos.persona_id == nadie.persona_id)",
        two_sources(),
    );
    let errs = row_of(&db, file).expect_err("an unknown row must refuse");
    assert!(
        errs.iter().any(|m| m.contains("nadie")),
        "the diagnostic must name the row that is not there, got {errs:?}"
    );
}

/// Two sources that share a column name JOIN, and both columns survive: the
/// qualified reference removes the ambiguity a collision rule would have
/// guarded, so there is no collision rule.
#[test]
fn two_sources_sharing_a_column_name_both_keep_it() {
    let (db, file) = db_with(
        "ventas := pedidos.join(personas, on = pedidos.persona_id == personas.persona_id)",
        vec![
            (
                "o.csv",
                &[
                    ("id", Primitive::Integer),
                    ("persona_id", Primitive::Integer),
                    ("nombre", Primitive::String),
                ],
            ),
            (
                "p.csv",
                &[
                    ("persona_id", Primitive::Integer),
                    ("nombre", Primitive::String),
                ],
            ),
        ],
    );
    assert_eq!(
        row_of(&db, file).expect("two sources sharing a column name now join"),
        ["id", "persona_id", "nombre", "persona_id", "nombre"]
    );
}

/// Two sources whose rows agree — what a `union` needs and a `join` does not
/// care about.
fn twin_sources() -> Vec<(&'static str, &'static [(&'static str, Primitive)])> {
    const COLUMNS: &[(&str, Primitive)] =
        &[("id", Primitive::Integer), ("nombre", Primitive::String)];
    vec![("o.csv", COLUMNS), ("p.csv", COLUMNS)]
}

/// A `union` keeps the row and REPLACES the names: its result is neither side,
/// so it answers to the pipeline and to nothing else.
///
/// This is the one place a derived relation introduces a binding its base did
/// not have. Every other verb carries, restricts or extends the names it was
/// given, which is why the scope is built by name at all — and why keeping the
/// left side's would be the ambiguity `select` after a `join` was decided
/// against on 2026-08-14: `pedidos.nombre` would name rows that came from
/// `personas`.
#[test]
fn a_union_answers_to_the_pipeline_and_to_neither_side() {
    let (db, file) = db_reading(
        "ventas := pedidos.union(personas)",
        "ventas",
        "nombre",
        twin_sources(),
    );
    assert_eq!(row_of(&db, file).expect("types"), ["id", "nombre"]);

    let (db, file) = db_reading(
        "ventas := pedidos.union(personas)",
        "pedidos",
        "nombre",
        twin_sources(),
    );
    let errs = row_of(&db, file).expect_err("the left side's name did not survive");
    assert!(
        errs.iter().any(|m| m.contains("pedidos")),
        "the diagnostic must name the row that is gone, got {errs:?}"
    );
}

/// The sides of a `union` are paired COLUMN BY COLUMN, and the refusal says
/// which pair disagreed.
///
/// `fossil-df` unions by position and hands back a schema with the left side's
/// names, so a mismatch it accepted would be a corpus whose `nombre` column
/// holds half integers. The rule is the backend's, said where a user hears it.
#[test]
fn a_union_whose_sides_disagree_is_refused() {
    let (db, file) = db_reading(
        "ventas := pedidos.union(personas)",
        "ventas",
        "id",
        two_sources(),
    );
    let errs = row_of(&db, file).expect_err("three columns against two must refuse");
    assert!(
        errs.iter().any(|m| m.contains("`union` in `ventas`")),
        "got {errs:?}"
    );

    let (db, file) = db_reading(
        "ventas := pedidos.union(personas)",
        "ventas",
        "id",
        vec![
            (
                "o.csv",
                &[("id", Primitive::Integer), ("nombre", Primitive::String)],
            ),
            (
                "p.csv",
                &[("id", Primitive::Integer), ("nombre", Primitive::Integer)],
            ),
        ],
    );
    let errs = row_of(&db, file).expect_err("a column of a different type must refuse");
    assert!(
        errs.iter().any(|m| m.contains("column 2")),
        "the refusal names the pair that disagreed, got {errs:?}"
    );
}

/// `distinct()` keeps rows, not columns — the row it hands on is the one it was
/// given — and it takes NOTHING.
///
/// The column list would be `DISTINCT ON`, whose surviving row is arbitrary
/// without an `ORDER BY`, and `sort` has no lowering to write one with. The
/// refusal is the binder's arity check, and `distinct` is the first verb to
/// reach it with an empty parameter list.
#[test]
fn distinct_keeps_the_row_and_takes_no_arguments() {
    let (db, file) = db_with("ventas := pedidos.distinct()", two_sources());
    assert_eq!(
        row_of(&db, file).expect("types"),
        ["id", "persona_id", "total"]
    );

    let (db, file) = db_with("ventas := pedidos.distinct(pedidos.id)", two_sources());
    let errs = row_of(&db, file).expect_err("a column list must refuse");
    assert!(
        errs.iter()
            .any(|m| m.contains("`distinct` in `ventas` takes no arguments")),
        "the refusal says the verb takes nothing, got {errs:?}"
    );
}
