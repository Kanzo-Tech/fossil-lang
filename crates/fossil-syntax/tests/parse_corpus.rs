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
    let mut out = String::new();
    let pad = "  ".repeat(depth);
    out.push_str(&format!("{pad}{:?}\n", node.kind()));
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
                out.push_str(&format!("{pad}{:?} {:?}\n", t.kind(), t.text()));
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
            expect_file![concat!("fixtures/", $bucket, "/", $stem, ".cst.txt")]
                .assert_eq(&cst);
        }
    };
}

// ─── Bucket 1: pipeline + postfix ─────────────────────────────────────
fixture_test!(pipe_basic_01,                "01_pipeline_postfix", "01_basic_pipe");
fixture_test!(pipe_chained_three_deep_02,   "01_pipeline_postfix", "02_chained_pipes_three_deep");
fixture_test!(pipe_partial_app_03,          "01_pipeline_postfix", "03_partial_app_with_underscore");
fixture_test!(pipe_missing_arg_04,          "01_pipeline_postfix", "04_missing_arg_recovers");
fixture_test!(pipe_trailing_05,             "01_pipeline_postfix", "05_trailing_pipe_recovers");
fixture_test!(pipe_lone_dot_06,             "01_pipeline_postfix", "06_malformed_field_ref_recovers");

// ─── Bucket 2: ternary + arithmetic ───────────────────────────────────
fixture_test!(ternary_simple_07,            "02_ternary_arithmetic", "07_ternary_simple");
fixture_test!(ternary_nested_right_08,      "02_ternary_arithmetic", "08_ternary_nested_right_assoc");
fixture_test!(full_precedence_walk_09,      "02_ternary_arithmetic", "09_full_precedence_walk");
fixture_test!(ternary_unbalanced_10,        "02_ternary_arithmetic", "10_unbalanced_ternary_recovers");
fixture_test!(lone_question_mark_11,        "02_ternary_arithmetic", "11_lone_question_mark_recovers");
fixture_test!(double_minus_unary_12,        "02_ternary_arithmetic", "12_double_minus_unary_recovers");

// ─── Bucket 3: mappings + annotations ─────────────────────────────────
fixture_test!(mapping_in_clause_13,         "03_mappings_annotations", "13_mapping_with_in_clause");
fixture_test!(mapping_shape_intersection_14,"03_mappings_annotations", "14_mapping_with_shape_intersection");
fixture_test!(property_nested_annot_15,     "03_mappings_annotations", "15_property_with_nested_annotation");
fixture_test!(mapping_missing_from_16,      "03_mappings_annotations", "16_mapping_missing_from_recovers");
fixture_test!(annotation_unclosed_17,       "03_mappings_annotations", "17_annotation_unclosed_brace_recovers");
fixture_test!(malformed_property_lhs_18,    "03_mappings_annotations", "18_malformed_property_lhs_recovers");

// ─── Bucket 4: prefix + IRI + triple ──────────────────────────────────
fixture_test!(use_selective_import_19,      "04_prefix_iri_triple", "19_use_with_selective_import");
fixture_test!(exported_def_type_annot_20,   "04_prefix_iri_triple", "20_exported_definition_with_type_annot");
fixture_test!(triple_term_object_21,        "04_prefix_iri_triple", "21_triple_term_in_object_position");
fixture_test!(broken_template_22,           "04_prefix_iri_triple", "22_broken_template_interpolation_recovers");
fixture_test!(unterminated_iri_23,          "04_prefix_iri_triple", "23_unterminated_iri_recovers");
fixture_test!(half_triple_term_24,          "04_prefix_iri_triple", "24_half_triple_term_recovers");

// ─── Bucket 5: top-level + indent ─────────────────────────────────────
fixture_test!(mixed_top_level_25,           "05_toplevel_indent", "25_mixed_top_level_items");
fixture_test!(record_literal_26,            "05_toplevel_indent", "26_record_literal_in_property_value");
fixture_test!(multi_line_annotation_27,     "05_toplevel_indent", "27_multi_line_annotation_body");
fixture_test!(inconsistent_dedent_28,       "05_toplevel_indent", "28_inconsistent_dedent_recovers");
fixture_test!(mapping_body_dedent_29,       "05_toplevel_indent", "29_mapping_body_de_indented_recovers");
fixture_test!(two_items_one_broken_30,      "05_toplevel_indent", "30_two_top_level_items_one_broken_other_fine");
