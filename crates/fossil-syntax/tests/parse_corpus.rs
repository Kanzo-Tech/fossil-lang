//! Parser corpus: every `tests/fixtures/**/*.fossil` is parsed, its CST is
//! compared with the `.cst.txt` beside it (`UPDATE_EXPECT=1` rewrites them),
//! and a fixture reports a diagnostic exactly when its name ends in
//! `_recovers`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use expect_test::expect_file;
use fossil_base::test_support::NativeSystem;
use fossil_base::{Diagnostic, FossilDb, SourceFile, System};
use fossil_syntax::{SyntaxKind, SyntaxNode, parse};

/// Parse `src`, render its CST without trivia, and count its diagnostics.
fn parse_fixture(src: &str) -> (String, usize) {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "fixture.fossil".to_string());
    let cst = parse(&db, file);
    let diagnostics = parse::accumulated::<Diagnostic>(&db, file).len();
    (render_node(&cst.root(&db).syntax(), 0), diagnostics)
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

fn fixtures(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("tests/fixtures") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            fixtures(&path, out);
        } else if path.extension().is_some_and(|e| e == "fossil") {
            out.push(path);
        }
    }
}

#[test]
fn every_fixture_parses_to_its_cst() {
    let mut paths = Vec::new();
    fixtures(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut paths,
    );
    assert!(!paths.is_empty());
    for path in paths {
        let src = std::fs::read_to_string(&path).expect("a fixture");
        let (cst, diagnostics) = parse_fixture(&src);
        expect_file![path.with_extension("cst.txt")].assert_eq(&cst);
        let recovers = path
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().ends_with("_recovers"));
        assert_eq!(
            diagnostics > 0,
            recovers,
            "{}: {diagnostics} diagnostics; a fixture reports one exactly when it is named `_recovers`",
            path.display()
        );
    }
}
