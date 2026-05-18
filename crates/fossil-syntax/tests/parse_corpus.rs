//! 30-fixture parser corpus driver — per RESEARCH.md §Q10.
//!
//! Each fixture pairs a `.fossil` source file with a `.cst.txt` snapshot.
//! Wave 0 (plan 02-01) shipped placeholder snapshots that Wave 1 plans
//! (02-02 + 02-03) regenerate via `UPDATE_EXPECT=1`.
//!
//! Per CLAUDE.md Style: parser CSTs use `expect-test` (NOT `insta`).

use std::sync::Arc;

use expect_test::expect_file;
use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_syntax::{SyntaxKind, SyntaxNode, parse};

/// Build a minimal Salsa db, run the parser, render the resulting CST as
/// a debug-format tree the snapshot files can compare against.
fn parse_to_cst_text(src: &str) -> String {
    let system: Arc<dyn System> = Arc::new(NativeSystem);
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
    pipe_partial_app_03,
    "01_pipeline_postfix",
    "03_partial_app_with_underscore"
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
fixture_test!(
    pipe_lone_dot_06,
    "01_pipeline_postfix",
    "06_malformed_field_ref_recovers"
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

// ─── Bucket 3: mappings + annotations ─────────────────────────────────
fixture_test!(
    mapping_in_clause_13,
    "03_mappings_annotations",
    "13_mapping_with_in_clause"
);
fixture_test!(
    mapping_shape_intersection_14,
    "03_mappings_annotations",
    "14_mapping_with_shape_intersection"
);
fixture_test!(
    property_nested_annot_15,
    "03_mappings_annotations",
    "15_property_with_nested_annotation"
);
fixture_test!(
    mapping_missing_from_16,
    "03_mappings_annotations",
    "16_mapping_missing_from_recovers"
);
fixture_test!(
    annotation_unclosed_17,
    "03_mappings_annotations",
    "17_annotation_unclosed_brace_recovers"
);
fixture_test!(
    malformed_property_lhs_18,
    "03_mappings_annotations",
    "18_malformed_property_lhs_recovers"
);

// ─── Bucket 4: prefix + IRI + triple ──────────────────────────────────
fixture_test!(
    use_selective_import_19,
    "04_prefix_iri_triple",
    "19_use_with_selective_import"
);
fixture_test!(
    exported_def_type_annot_20,
    "04_prefix_iri_triple",
    "20_exported_definition_with_type_annot"
);
fixture_test!(
    triple_term_object_21,
    "04_prefix_iri_triple",
    "21_triple_term_in_object_position"
);
fixture_test!(
    broken_template_22,
    "04_prefix_iri_triple",
    "22_broken_template_interpolation_recovers"
);
fixture_test!(
    unterminated_iri_23,
    "04_prefix_iri_triple",
    "23_unterminated_iri_recovers"
);
fixture_test!(
    half_triple_term_24,
    "04_prefix_iri_triple",
    "24_half_triple_term_recovers"
);

// ─── Bucket 5: top-level + indent ─────────────────────────────────────
fixture_test!(
    mixed_top_level_25,
    "05_toplevel_indent",
    "25_mixed_top_level_items"
);
fixture_test!(
    record_literal_26,
    "05_toplevel_indent",
    "26_record_literal_in_property_value"
);
fixture_test!(
    multi_line_annotation_27,
    "05_toplevel_indent",
    "27_multi_line_annotation_body"
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

// =====================================================================
// Diagnostic-accumulator coverage for recovery fixtures (plan 02-03 Task 3)
// =====================================================================
//
// The 30 `fixture_test!`s above verify the SHAPE of the resulting CST
// (snapshot tests). This test verifies the BLAME PATH — that every
// recovery fixture actually pushes a `Diagnostic` into the public
// accumulator via the wrapping `parse()` Salsa query (RESEARCH.md §Q11
// wiring). The LSP (Phase 6) consumes diagnostics through exactly this
// accumulator, so the recovery + diagnostic plumbing must be exercised
// independently of CST shape.
//
// Coverage: 15 of the 18 corpus recovery fixtures emit ≥1 diagnostic.
// The remaining 3 are documented exceptions (see plan 02-03 SUMMARY):
//
//   - `12_double_minus_unary_recovers.fossil` (`iri = - - x`) parses
//     cleanly under the grammar: unary `-` is right-associative L8
//     (grammar.bnf §OPERATOR PRECEDENCE TABLE), so `- - x` is the valid
//     `UNARY(MINUS, UNARY(MINUS, x))` tree. The fixture name reflects
//     a Wave 0 (plan 02-01) over-eager labeling; the parser correctly
//     does NOT emit an error here.
//
//   - `22_broken_template_interpolation_recovers.fossil`
//     (`iri = \`prefix${.id\``): the unterminated `${...}` is INSIDE
//     the TEMPLATE token, which the lexer matches atomically per
//     grammar.bnf line 42 (`INTERPOLATION := '${' Expression '}'` is
//     deferred to Phase 4 per plan 02-02 expr.rs comment). The parser
//     sees a complete TEMPLATE token and parses cleanly. Phase 4 will
//     lift interpolation parsing and add the diagnostic.
//
//   - (the third "happy-path-with-recovery edge case" enumerated in the
//     plan's note is fixture 12 itself; only 2 fixtures actually skip
//     the diagnostic gate. The plan's "remaining 3" comment counted
//     `06_malformed_field_ref_recovers` as a fixture that might not
//     emit, but on inspection that one DOES emit 1 ERROR + 1 diagnostic.)

use fossil_base::Diagnostic;
use salsa::Accumulator;

#[test]
fn recovery_fixtures_each_emit_at_least_one_diagnostic() {
    // 13 fixtures expected to emit ≥1 diagnostic. See module-level
    // comment above for why fixtures 12 + 22 are excluded.
    let recovery_fixtures: &[(&str, &str)] = &[
        ("01_pipeline_postfix", "04_missing_arg_recovers"),
        ("01_pipeline_postfix", "05_trailing_pipe_recovers"),
        ("01_pipeline_postfix", "06_malformed_field_ref_recovers"),
        ("02_ternary_arithmetic", "10_unbalanced_ternary_recovers"),
        ("02_ternary_arithmetic", "11_lone_question_mark_recovers"),
        (
            "03_mappings_annotations",
            "16_mapping_missing_from_recovers",
        ),
        (
            "03_mappings_annotations",
            "17_annotation_unclosed_brace_recovers",
        ),
        (
            "03_mappings_annotations",
            "18_malformed_property_lhs_recovers",
        ),
        ("04_prefix_iri_triple", "23_unterminated_iri_recovers"),
        ("04_prefix_iri_triple", "24_half_triple_term_recovers"),
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
        let system: Arc<dyn System> = Arc::new(NativeSystem);
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
