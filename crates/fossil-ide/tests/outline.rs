//! Snapshot test for `fossil_ide::document_symbols` (LSP-01 / SC#5).
//!
//! Renders the document-symbol outline (a flat `Vec<DocumentSymbol>` in v0.1)
//! into a stable `kind name @ start-end` table so the snapshot diffs readably
//! when the SymbolIndex walk or the kind mapping changes.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_ide::document_symbols;
use lsp_types::SymbolKind;

/// A multi-item fixture: a prefix decl, a source def, two mappings (each with a
/// shape ref), and an exported function definition.
const FIXTURE: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")

@export greet := .name

User : ex:Person from users
    ex:name = .name

Org : ex:Organization from users
    ex:title = .title
";

fn kind_name(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::NAMESPACE => "namespace",
        SymbolKind::CLASS => "class",
        SymbolKind::FUNCTION => "function",
        SymbolKind::INTERFACE => "interface",
        _ => "other",
    }
}

fn render(src: &str) -> String {
    let system: Arc<dyn System> = Arc::new(NativeSystem);
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "fixture.fossil".to_string());
    let mut out = String::new();
    for s in document_symbols(&db, file) {
        out.push_str(&format!(
            "{:<10} {:<16} @ {}:{}-{}:{}\n",
            kind_name(s.kind),
            s.name,
            s.range.start.line,
            s.range.start.character,
            s.range.end.line,
            s.range.end.character,
        ));
    }
    out
}

#[test]
fn outline_snapshot() {
    insta::assert_snapshot!(render(FIXTURE));
}
