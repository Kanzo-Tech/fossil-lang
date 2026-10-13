//! Snapshot test for `fossil_ide::document_symbols`.
//!
//! Renders the document-symbol outline (a flat `Vec<DocumentSymbol>`) into a
//! stable `kind name @ range / selection` table so the snapshot diffs readably
//! when the def map's definitions or the kind mapping change.

use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_ide::document_symbols;
use lsp_types::SymbolKind;

/// A multi-item fixture: a type binding of two names, a source binding, and two
/// mappings. The two types share their binding's range and each has its own
/// selection; `outline.rs`'s `kind_of` emits CLASS, INTERFACE and VARIABLE and
/// nothing else, which is why `kind_name` names only those three.
const FIXTURE: &str = "\
type { Person, Organization } := io.shex(\"org.shex\")

users := io.csv(\"users.csv\")

User : Person from users
    name = users.name

Org : Organization from users
    title = users.title
";

const fn kind_name(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::CLASS => "class",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::VARIABLE => "variable",
        _ => "other",
    }
}

fn render(src: &str) -> String {
    use std::fmt::Write as _;
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "fixture.fossil".to_string());
    let mut out = String::new();
    for s in document_symbols(&db, file) {
        let _ = writeln!(
            out,
            "{:<10} {:<16} @ {}:{}-{}:{} / {}:{}-{}:{}",
            kind_name(s.kind),
            s.name,
            s.range.start.line,
            s.range.start.character,
            s.range.end.line,
            s.range.end.character,
            s.selection_range.start.line,
            s.selection_range.start.character,
            s.selection_range.end.line,
            s.selection_range.end.character,
        );
    }
    out
}

#[test]
fn outline_snapshot() {
    expect_test::expect_file!["snapshots/outline_snapshot.txt"].assert_eq(&render(FIXTURE));
}
