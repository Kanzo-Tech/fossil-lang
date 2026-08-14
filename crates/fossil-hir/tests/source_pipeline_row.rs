//! The row algebra of a source pipeline: what `where`, `select`
//! and `join` do to the type of a row.
//!
//! Only this crate can test it, because it needs a real descriptor behind the
//! base binding — a pipeline over a source that declares no columns has no row
//! to transform, and the interesting refusals (a column no side has, a binding
//! the pipeline never drew on) are exactly the ones an untyped source cannot
//! raise.
//!
//! # The spelling, and the one assertion that did not survive it
//!
//! This file was written in four spellings the language no longer has — the
//! `a |> f()` pipeline, the leading-dot `.id`, a shape as a CURIE against a
//! `prefix` declaration, and a mapping body with no `@subject`. Three of them
//! are a substitution: `a.f()`, `pedidos.id`, a name a `type { … } := io.shex(…)`
//! binding introduced, and an identity on the body's first line.
//!
//! The fourth was not, and it is why the file waited. It asserted that the
//! joined row holds `persona_id` ONCE, which came from `on = .persona_id` —
//! `USING (k)`, the key named once because both sides were assumed to spell it
//! the same. Ruling 17 of `SURFACE-PLAN.md` deleted that, and with it the
//! collision rule, on the promise that qualification would do the work instead:
//! `crate::infer`'s `RowScope` keeps a join's two sides as two entries, each
//! under its own binding, and two columns called `persona_id` are two distinct
//! columns. So the joined row holds it TWICE, once per side, and `select` is
//! what narrows to the one you meant — which is the other half of the same
//! ruling, decided as open question 4 of `grammar.bnf, § OPEN` on 2026-08-14:
//! `select` may follow a `join`, and it names a QUALIFIED column.
//!
//! # What this file no longer proves, and nothing else does either
//!
//! Two of the old assertions went with the same ruling, and only one of them
//! was replaced:
//!
//!   - «a name that would appear twice is refused» is the collision rule, and
//!     it is DELETED. `two_sources_sharing_a_column_name_both_keep_it` below is
//!     the opposite assertion, which is the thing that is true now.
//!   - «the key must be the same type on both sides» has no test here any more,
//!     and it has no CHECK any more. `crate::infer::apply_source_op` runs
//!     `check_refs` over a `join`'s condition — which asks only that every
//!     column it names exists under the binding that qualifies it — and nothing
//!     types the condition. `on = .k` was a KEY, and one column compared with
//!     itself is where "the same type on both sides" came from; ruling 17 made
//!     it an ordinary predicate and the rule was not rewritten for the new
//!     shape. So `pedidos.persona_id == personas.persona_id` with an `Integer`
//!     on one side and a `String` on the other type-checks today. Asserting
//!     that here would pin the gap as the contract, so it is written down
//!     instead.

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
/// test, and a mapping that reads it.
///
/// The identity is a required assignment on the body's first line, and naming a
/// shape document is mandatory — a program missing either does not reach the
/// row algebra at all, so neither is optional scaffolding here.
fn program(pipe: &str) -> String {
    format!(
        "type {{ Persona }} := io.shex(\"v.shex\")\n\
         pedidos := io.csv(\"o.csv\")\n\
         personas := io.csv(\"p.csv\")\n\
         {pipe}\n\
         Venta : Persona from ventas\n    \
         @subject = \"https://example.org/v/{{pedidos.id}}\"\n    \
         name = pedidos.id\n"
    )
}

fn db_with(
    pipe: &str,
    descriptors: Vec<(&'static str, &'static [(&'static str, Primitive)])>,
) -> (FossilDb, SourceFile) {
    let (db, file) = db_with_document(&program(pipe), "v.shex", DOCUMENT);
    for (uri, columns) in descriptors {
        register_inferred(&db, uri, columns);
    }
    (db, file)
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
/// old spelling `on = .persona_id` meant `USING (k)` and identified them;
/// ruling 17 replaced it with an equality that names both sides, and a
/// predicate relating two columns does not merge them.
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
/// This is the half of the row algebra that ruling 17 promised and open
/// question 4 delivered. `HirSourceOp::Select` carried column names only, so a
/// name was looked for in the rows in order and taken from the first that had
/// it — which made `select(persona_id)` mean the left side's for a reason the
/// author never wrote, and which is why `select` after a `join` was left open.
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
/// and the message says which — the refusal that replaced the collision rule.
///
/// Ruling 17 deleted "a name that would appear twice is an error" outright
/// rather than relaxing it: the body writes `pedidos.nombre` next to
/// `personas.nombre`, so a shared column name means nothing and two sources that
/// share one now join. What is left to get wrong is the QUALIFIER, and
/// `on = pedidos.persona_id == nadie.id` used to pass because "on either side,
/// under any name" was the question being asked.
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

/// Two sources that share a column name JOIN, and both columns survive — the
/// program ruling 17 changed the meaning of, asserted from the row side.
///
/// The old test here refused this and its message named the colliding column.
/// The rule it enforced existed to remove an ambiguity that the qualified
/// reference removed instead, so it is deleted rather than relaxed, and what
/// takes its place is the opposite assertion.
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
