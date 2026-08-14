//! Parser corpus driver. Eighteen fixtures; it was
//! thirty until the forms the HIR never read left the grammar, then twenty,
//! and then three more went with the CURIE and the backtick.
//!
//! Every fixture is written in the spelling `grammar.bnf` specifies, and the
//! rewrite was a rewrite rather than a patch: a shape is a bare name resolved
//! against a `type { … } := …` binding, a reference is qualified (`User.name`),
//! an identity is a quoted string with `{expr}` holes, and no file declares a
//! prefix. The one thing NOT rewritten is `|>`: ruling 7 of 2026-08-11 kills it
//! and step 6 of `SURFACE-PLAN.md` removes it, so bucket 1 spells what the
//! parser still accepts.
//!
//! # The three that were deleted rather than rewritten
//!
//! A fixture whose whole subject is a form the language no longer has proves
//! nothing once the form is gone, and rewriting it into a different subject
//! would be a fixture pretending to a history it does not have:
//!
//! - `01_pipeline_postfix/06_malformed_field_ref_recovers` — a lone `.`,
//!   recovering a `FieldRef`. There is no `FieldRef`: every reference is
//!   qualified, so a leading `.` names a column of a row with no name and is
//!   refused by name, and
//!   `items::disambiguation::a_leading_dot_is_refused_and_names_the_qualified_form`
//!   pins the refusal and its span.
//! - `04_prefix_iri_triple/22_broken_template_interpolation_recovers` — an
//!   unterminated hole in a BACKTICK template. The backtick is not a token
//!   at all. The live half of what it covered — a hole that
//!   never closes — is `03_mappings_annotations/19_unterminated_interpolation_recovers`,
//!   in the quoted spelling.
//! - `04_prefix_iri_triple/23_unterminated_iri_recovers` — an unterminated
//!   `<https://…` on a `prefix` line: two dead forms in one file. There is no
//!   vocabulary declaration to open and no absolute-IRI token to leave
//!   unterminated — `<` is the comparison operator and nothing else.
//!
//! Bucket 4 held nothing else, so the bucket went with them.
//!
//! Each fixture pairs a `.fossil` source file with a `.cst.txt` snapshot.
//!
//! Per CLAUDE.md Style: parser CSTs use `expect-test` (NOT `insta`).

use std::sync::Arc;

use expect_test::expect_file;
use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_syntax::{SyntaxKind, SyntaxNode, parse};

/// Build a minimal Salsa db, run the parser, render the resulting CST as
/// a debug-format tree the snapshot files can compare against.
fn parse_to_cst_text(src: &str) -> String {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "fixture.fossil".to_string());
    let cst = parse(&db, file);
    render_node(&cst.root(&db).syntax(), 0)
}

/// Recursively render a `SyntaxNode` with 2-space indentation per depth,
/// dropping trivia (WHITESPACE / NEWLINE / COMMENT / INDENT / DEDENT) for
/// snapshot stability — incidental whitespace would make snapshots brittle.
fn render_node(node: &SyntaxNode, depth: usize) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let pad = "  ".repeat(depth);
    writeln!(out, "{pad}{:?}", node.kind()).expect("writing to String never fails");
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(n) => {
                out.push_str(&render_node(&n, depth + 1));
            }
            rowan::NodeOrToken::Token(t) => {
                if matches!(
                    t.kind(),
                    SyntaxKind::WHITESPACE
                        | SyntaxKind::NEWLINE
                        | SyntaxKind::COMMENT
                        | SyntaxKind::INDENT
                        | SyntaxKind::DEDENT
                ) {
                    continue;
                }
                let pad = "  ".repeat(depth + 1);
                writeln!(out, "{pad}{:?} {:?}", t.kind(), t.text())
                    .expect("writing to String never fails");
            }
        }
    }
    out
}

macro_rules! fixture_test {
    ($name:ident, $bucket:literal, $stem:literal) => {
        #[test]
        fn $name() {
            let src = include_str!(concat!("fixtures/", $bucket, "/", $stem, ".fossil"));
            let cst = parse_to_cst_text(src);
            expect_file![concat!("fixtures/", $bucket, "/", $stem, ".cst.txt")].assert_eq(&cst);
        }
    };
}

// ─── Bucket 1: pipeline + postfix ─────────────────────────────────────
fixture_test!(pipe_basic_01, "01_pipeline_postfix", "01_basic_pipe");
fixture_test!(
    pipe_chained_three_deep_02,
    "01_pipeline_postfix",
    "02_chained_pipes_three_deep"
);
fixture_test!(
    pipe_missing_arg_04,
    "01_pipeline_postfix",
    "04_missing_arg_recovers"
);
fixture_test!(
    pipe_trailing_05,
    "01_pipeline_postfix",
    "05_trailing_pipe_recovers"
);

// ─── Bucket 2: ternary + arithmetic ───────────────────────────────────
fixture_test!(
    ternary_simple_07,
    "02_ternary_arithmetic",
    "07_ternary_simple"
);
fixture_test!(
    ternary_nested_right_08,
    "02_ternary_arithmetic",
    "08_ternary_nested_right_assoc"
);
fixture_test!(
    full_precedence_walk_09,
    "02_ternary_arithmetic",
    "09_full_precedence_walk"
);
fixture_test!(
    ternary_unbalanced_10,
    "02_ternary_arithmetic",
    "10_unbalanced_ternary_recovers"
);
fixture_test!(
    lone_question_mark_11,
    "02_ternary_arithmetic",
    "11_lone_question_mark_recovers"
);
fixture_test!(
    double_minus_unary_12,
    "02_ternary_arithmetic",
    "12_double_minus_unary_recovers"
);

// ─── Bucket 3: mappings ───────────────────────────────────────────────
fixture_test!(
    mapping_missing_from_16,
    "03_mappings_annotations",
    "16_mapping_missing_from_recovers"
);
fixture_test!(
    malformed_property_lhs_18,
    "03_mappings_annotations",
    "18_malformed_property_lhs_recovers"
);
// A hole that never closes, in the ONE string spelling. It replaces the
// backtick fixture in the deleted bucket 4: `carve_interpolations` carves the
// literal whether or not the hole closes, and the missing `}` reaches
// `expect_or_recover(RBRACE, [STRING_CLOSE])` — live code that would otherwise
// have lost its only fixture to a deletion about the delimiter.
fixture_test!(
    unterminated_interpolation_19,
    "03_mappings_annotations",
    "19_unterminated_interpolation_recovers"
);

// ─── Bucket 4 is gone ─────────────────────────────────────────────────
// It was `04_prefix_iri_triple/` — imports, IRI templates and RDF 1.2 triple
// terms. The triple term went with the RDF-specific surface, the import with
// the module system there never was, and the prefix and the IRI template with
// the CURIE. See the module header for the two fixtures and what each proved.

// ─── Bucket 5: top-level + indent ─────────────────────────────────────
fixture_test!(
    mixed_top_level_25,
    "05_toplevel_indent",
    "25_mixed_top_level_items"
);
fixture_test!(
    inconsistent_dedent_28,
    "05_toplevel_indent",
    "28_inconsistent_dedent_recovers"
);
fixture_test!(
    mapping_body_dedent_29,
    "05_toplevel_indent",
    "29_mapping_body_de_indented_recovers"
);
fixture_test!(
    two_items_one_broken_30,
    "05_toplevel_indent",
    "30_two_top_level_items_one_broken_other_fine"
);
// `type` is contextual, and this fixture is the proof: the same file uses it
// as the type binder AND as an ordinary binding name. If `type` were ever
// reserved, the second line stops parsing — which matters in a language whose
// commonest predicate is `rdf:type`.
fixture_test!(
    type_def_and_type_as_a_name_31,
    "05_toplevel_indent",
    "31_type_def_and_type_as_a_binding_name"
);

// =====================================================================
// Diagnostic-accumulator coverage for recovery fixtures (plan 02-03 Task 3)
// =====================================================================
//
// The `fixture_test!`s above verify the SHAPE of the resulting CST
// (snapshot tests). This test verifies the BLAME PATH — that every
// recovery fixture actually pushes a `Diagnostic` into the public
// accumulator via the wrapping `parse()` Salsa query. The LSP consumes
// diagnostics through exactly this
// accumulator, so the recovery + diagnostic plumbing must be exercised
// independently of CST shape.
//
// Coverage: 9 of the 10 corpus recovery fixtures emit ≥1 diagnostic.
// The remaining 1 is a documented exception:
//
//   - `12_double_minus_unary_recovers.fossil` (`@subject = - - x`) parses
//     cleanly: unary `-` is right-associative (L7 in grammar.bnf
//     §OPERATOR PRECEDENCE TABLE, L8 in the parser's own table, which
//     still counts `|>`), so `- - x` is the valid
//     `UNARY(MINUS, UNARY(MINUS, x))` tree. The fixture name reflects
//     a Wave 0 (plan 02-01) over-eager labeling; the parser correctly
//     does NOT emit an error here.

use fossil_base::Diagnostic;
use salsa::Accumulator;

#[test]
fn recovery_fixtures_each_emit_at_least_one_diagnostic() {
    // 9 fixtures expected to emit ≥1 diagnostic. See the module-level
    // comment above for why fixture 12 is the only one excluded.
    let recovery_fixtures: &[(&str, &str)] = &[
        ("01_pipeline_postfix", "04_missing_arg_recovers"),
        ("01_pipeline_postfix", "05_trailing_pipe_recovers"),
        ("02_ternary_arithmetic", "10_unbalanced_ternary_recovers"),
        ("02_ternary_arithmetic", "11_lone_question_mark_recovers"),
        (
            "03_mappings_annotations",
            "16_mapping_missing_from_recovers",
        ),
        (
            "03_mappings_annotations",
            "18_malformed_property_lhs_recovers",
        ),
        (
            "03_mappings_annotations",
            "19_unterminated_interpolation_recovers",
        ),
        ("05_toplevel_indent", "28_inconsistent_dedent_recovers"),
        ("05_toplevel_indent", "29_mapping_body_de_indented_recovers"),
        (
            "05_toplevel_indent",
            "30_two_top_level_items_one_broken_other_fine",
        ),
    ];

    let crate_dir = env!("CARGO_MANIFEST_DIR");
    for (bucket, stem) in recovery_fixtures {
        let path = format!("{crate_dir}/tests/fixtures/{bucket}/{stem}.fossil");
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, src, path.clone());

        // Pull diagnostics accumulated by the Salsa `parse()` query.
        // Salsa 0.26's `accumulated::<A>(db, input)` returns `Vec<&A>`.
        let diags: Vec<&Diagnostic> = parse::accumulated::<Diagnostic>(&db, file);
        assert!(
            !diags.is_empty(),
            "recovery fixture {bucket}/{stem}: expected ≥1 Diagnostic in the accumulator, got 0",
        );
    }

    // Cross-check that the Accumulator trait is what we expect (compiles
    // even when no diagnostic exists — paranoia about a future Salsa
    // version renaming the trait surface). The helper is defined at module
    // scope (`assert_accumulator_bound` below) to keep clippy's
    // `items_after_statements` lint happy.
    assert_accumulator_bound::<Diagnostic>();
}

/// Compile-time witness that `Diagnostic` (and any other type we may want
/// to accumulate in tests) implements [`Accumulator`]. Used by the
/// recovery-fixture test as a paranoia check against a future Salsa version
/// renaming the trait surface.
const fn assert_accumulator_bound<A: Accumulator>() {}

/// NO FIXTURE SPELLS A RETIRED FORM.
///
/// The instruction for this corpus was «rewritten, not patched», and a rewrite
/// is exactly the kind of claim that rots: a `UPDATE_EXPECT=1` run will happily
/// bake `prefix ex: <…>` into a snapshot as an ERROR node and the suite goes
/// green with a corpus written in the dead language.
///
/// This is the guard for that, and it is the strongest one available here: it
/// does not grep for spellings, it parses every fixture and asserts that not
/// one of them produces a `RetiredSpelling` diagnostic. If a rewrite missed a
/// CURIE, the parser says so and this test reads the parser.
///
/// What it CANNOT prove: that a fixture is a form the language HAS. A file that
/// parses clean may still be nonsense the checker refuses — `@subject = ?` in
/// bucket 2 is a parse-recovery fixture on purpose. Only step 8's compilation
/// of `apps/docs/programs/` proves that, and this crate has no checker to ask.
#[test]
fn no_fixture_spells_a_retired_form() {
    use fossil_syntax::parser::diag::retired;

    let retired_messages = [
        retired::PREFIX_DECL,
        retired::CURIE,
        retired::ABSOLUTE_IRI,
        retired::LEADING_DOT,
        retired::BACKTICK,
    ];

    let root = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0usize;
    let mut stack = vec![std::path::PathBuf::from(&root)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}")) {
            let path = entry.expect("a readable dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "fossil") {
                continue;
            }
            let src =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
            let system: Arc<dyn System> = Arc::new(NativeSystem::default());
            let db = FossilDb::new(system);
            let file = SourceFile::new(&db, src, path.display().to_string());
            let _cst = parse(&db, file);
            for d in parse::accumulated::<Diagnostic>(&db, file) {
                assert!(
                    !retired_messages.iter().any(|m| d.message == *m),
                    "{}: this fixture still spells a form grammar.bnf retired — \
                     rewrite it, do not snapshot the refusal.\n  {}",
                    path.display(),
                    d.message,
                );
            }
            checked += 1;
        }
    }
    assert_eq!(
        checked, 18,
        "expected 18 fixtures; a file added or deleted without updating this \
         count means the `fixture_test!` table below has drifted from the disk",
    );
}
