//! **What a stage of a source pipeline is told when it does not lower.**
//!
//! `crate::lower::lower_source_stage` used to dispatch on the verb's TEXT, with
//! three arms and a catch-all, and the catch-all said one thing about two
//! different situations. A user writing `pedidos.sort(pedidos.id)` was told
//! `sort` *"is not a relation verb fossil lowers"* — false twice over: `sort`
//! IS a relation verb, the catalogue declares it with a `PlanOp` of its own,
//! and what it lacks is a lowering. The message even read its COUNT from the
//! registry while reading its LIST of implemented verbs from a string literal,
//! so half of it could go stale without the other half moving.
//!
//! The two situations are two answers now, and this file is what holds them
//! apart. It is the same distinction `catalogue.bnf`'s header argues for the
//! `io.` half — *"declared but not linked here" and "no such name" are
//! different answers* — arrived at from the other side of the catalogue.
//!
//! # What this file does NOT prove
//!
//! - **That the eight unlowered verbs should stay unlowered.** The gap is
//!   deliberate — the catalogue already carries their signatures, so the day one
//!   gains a lowering its arguments are checked by the code that checks
//!   `str.trim`'s. This pins the message, not the gap.
//! - **That a fourteenth `PlanOp` is impossible.** That is the compiler's, not
//!   this file's: the `match` in `lower_source_stage` is exhaustive and has no
//!   wildcard, so a new variant fails to build. A test cannot assert a
//!   compile error, and a test that tried would be asserting its own mock.
//! - **That the lowered verbs do the right thing.** `source_pipeline_row.rs`
//!   owns the row algebra; this owns the refusals.

use fossil_base::test_support::{db_with_document, register_inferred};
use fossil_base::{Diagnostic, FossilDb, SourceFile};
use fossil_graph_schema::Primitive;
use fossil_hir::lower::lower_to_hir;

const DOCUMENT: &str = "\
shape http://example.org/Persona
prop http://example.org/name - 1 1
";

const COLUMNS: &[(&str, Primitive)] = &[
    ("id", Primitive::Integer),
    ("persona_id", Primitive::Integer),
];

/// A program whose only interesting line is the pipeline under test.
///
/// Naming a shape document is mandatory and the identity is a required
/// assignment on the body's first line, so neither is optional scaffolding —
/// a program missing either never reaches the lowering of a pipeline at all.
fn program(pipe: &str) -> String {
    format!(
        "type {{ Persona }} := io.shex(\"v.shex\")\n\
         pedidos := io.csv(\"o.csv\")\n\
         personas := io.csv(\"p.csv\")\n\
         {pipe}\n\
         Venta : Persona from derived\n    \
         @subject = \"https://example.org/v/{{pedidos.id}}\"\n    \
         name = pedidos.id\n"
    )
}

fn diagnostics(pipe: &str) -> Vec<String> {
    let (db, file): (FossilDb, SourceFile) = db_with_document(&program(pipe), "v.shex", DOCUMENT);
    register_inferred(&db, "o.csv", COLUMNS);
    register_inferred(&db, "p.csv", COLUMNS);
    let _ = lower_to_hir(&db, file);
    lower_to_hir::accumulated::<Diagnostic>(&db, file)
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// The message names one message and only one, so a test that greps a list of
/// twenty cannot pass by accident.
fn one_about(pipe: &str, needle: &str) -> String {
    let all = diagnostics(pipe);
    let hits: Vec<&String> = all.iter().filter(|m| m.contains(needle)).collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one diagnostic containing {needle:?}, got {}:\n{all:#?}",
        hits.len()
    );
    hits[0].clone()
}

#[test]
fn a_catalogued_verb_with_no_lowering_says_which_of_the_two_things_is_true() {
    let m = one_about(
        "derived := pedidos.sort(pedidos.id)",
        "the catalogue declares",
    );
    assert!(
        m.contains("`sort` is a relation verb the catalogue declares"),
        "the verb has to be named, and named as catalogued: {m}"
    );
    assert!(
        !m.contains("is not a relation verb"),
        "this is the sentence that was false, and it must not come back: {m}"
    );
}

#[test]
fn the_message_lists_the_implemented_verbs_from_the_registry() {
    let m = one_about("derived := pedidos.sort(pedidos.id)", "Implemented today");
    // Alphabetical because `verb_names` sorts, and all five because the filter
    // is over the table rather than over a literal. It read `join`, `select`,
    // `where` until `distinct` and `union` gained lowerings, and then `group_by`
    // arrived and moved it again — which is the whole reason the list is
    // derived. Six of them is the whole of the direction `design/algebra`
    // declares.
    assert!(
        m.contains("Implemented today: `distinct`, `group_by`, `join`, `select`, `union`, `where`"),
        "the list must come from the registry, not from a format string: {m}"
    );
}

#[test]
fn a_name_the_catalogue_does_not_have_is_a_different_answer() {
    let m = one_about(
        "derived := pedidos.slugify(pedidos.id)",
        "is not a relation verb",
    );
    assert!(
        m.contains("`slugify` is not a relation verb"),
        "an uncatalogued name is refused by name: {m}"
    );
    // And it is told what there IS, from the table — twelve names, not a count
    // and not three of them. It named `aggregate` until `group_by` took the
    // aggregations into itself and that row was deleted; `count` is the
    // catalogued-and-unlowered one it stands for now.
    assert!(
        m.contains("`sort`") && m.contains("`where`") && m.contains("`count`"),
        "the catalogue's own list is what a reader needs here: {m}"
    );
}

#[test]
fn a_joins_condition_is_written_with_its_name() {
    // `ParamSpec::named` exists for this line. Without it the binder would fill
    // `on` from the second positional slot and this program would start
    // compiling — a widening of the language arrived at by accident.
    let m = one_about(
        "derived := pedidos.join(personas, pedidos.persona_id == personas.persona_id)",
        "needs `on =",
    );
    assert!(
        m.contains("`join` in `derived` needs `on = <predicate>`"),
        "the refusal names the parameter and its spelling: {m}"
    );
}

#[test]
fn the_example_in_a_message_names_the_binding_the_program_wrote() {
    // Rendered from the signature, not hardcoded. The old messages said
    // `e.g. select(User.id, User.name)` in a program that has no `User`.
    let m = one_about("derived := pedidos.select()", "select");
    assert!(
        m.contains("pedidos."),
        "an example that names a binding the program does not have is noise: {m}"
    );
    assert!(
        !m.contains("User."),
        "`User` is the hardcoded example, and no program here declares one: {m}"
    );
}
#[test]
fn a_union_refuses_the_alias_a_join_takes() {
    // `binding("other")` is the same parameter kind a join's right side is, and
    // `X as Y` fills one on its own — so without this arm the alias would be
    // read and dropped, which is the silent half of a wrong answer. A union's
    // result answers to one name and it is not the alias.
    let m = one_about("derived := pedidos.union(personas as otras)", "union");
    assert!(
        m.contains("alias `otras`") && m.contains("`derived`"),
        "the refusal names the alias and the name the result does answer to: {m}"
    );
}

/// **A `group_by` with nothing aggregated is refused.**
///
/// This is the rule that keeps the sixth verb from being a third spelling of
/// something the language has: `group_by(keys)` alone has exactly the rows of
/// `select(keys).distinct()`, and both of those lower already. So the
/// aggregations are not an optional position — the row declares them
/// `Arity::OneOrMore`, and `bind_stage_args` refuses the call out of the
/// signature, before any arm of the lowering sees it.
///
/// Proved red by declaring the position `Arity::Optional`: the program compiles
/// CLEAN, 0 passed and 1 failed, and the count of diagnostics containing
/// `group_by` goes 1 to 0. That zero is the shape of the defect — not a wrong
/// message, no message.
///
/// A hand-written emptiness check stood here first and was deleted for being
/// the second refusal of one mistake: removing it left the count at 1, because
/// the binder's own too-few-arguments arm had it covered.
#[test]
fn a_group_by_with_nothing_aggregated_is_refused() {
    let m = one_about("derived := pedidos.group_by(pedidos.id)", "group_by");
    assert!(
        m.contains("needs one or more aggregations and this call gives none"),
        "the position is named as the row declares it: {m}"
    );
    assert!(
        m.contains("total = math.sum(pedidos.amount)"),
        "and the example shows an aggregation, rendered from the row: {m}"
    );
}

/// An aggregation has to be an AGGREGATE, and the four are read from the
/// catalogue rather than listed here — the same derivation
/// `the_message_lists_the_implemented_verbs_from_the_registry` pins for verbs.
#[test]
fn a_group_by_refuses_a_call_that_is_not_an_aggregate() {
    let m = one_about(
        "derived := pedidos.group_by(pedidos.id, n = str.length(pedidos.id))",
        "is not an aggregate",
    );
    assert!(
        m.contains("`str.length` is not an aggregate"),
        "the call is refused by name: {m}"
    );
    assert!(
        m.contains("`math.avg`") && m.contains("`math.sum`"),
        "and the aggregates come from the table: {m}"
    );
}
