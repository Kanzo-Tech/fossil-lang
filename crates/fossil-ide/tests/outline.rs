//! Snapshot test for `fossil_ide::document_symbols` (LSP-01 / SC#5).
//!
//! Renders the document-symbol outline (a flat `Vec<DocumentSymbol>` in v0.1)
//! into a stable `kind name @ start-end` table so the snapshot diffs readably
//! when the `SymbolIndex` walk or the kind mapping changes.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_ide::document_symbols;
use lsp_types::SymbolKind;

/// A multi-item fixture: a type binding, a source def, and two mappings (each
/// with a shape ref).
///
/// It opened with `prefix ex: <https://example.org/>`, and the outline carried a
/// NAMESPACE for it. There is no vocabulary declaration and so no namespace to
/// outline — `outline.rs`'s `map_kind` emits CLASS, INTERFACE and VARIABLE and
/// nothing else, which is why `kind_name` names only those three.
///
/// `users` is the third: the `:=` binding on line 2 was in NO outline until
/// `SymbolIndex` grew its `SOURCE_DEF` arm, because the walk tested only for
/// `MAPPING`. The snapshot below gained exactly that row.
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
            "{:<10} {:<16} @ {}:{}-{}:{}",
            kind_name(s.kind),
            s.name,
            s.range.start.line,
            s.range.start.character,
            s.range.end.line,
            s.range.end.character,
        );
    }
    out
}

#[test]
fn outline_snapshot() {
    insta::assert_snapshot!(render(FIXTURE));
}
