//! `textDocument/documentSymbol` — the document outline (SC#5).
//!
//! Produces the hierarchical-capable symbol list a client renders in its
//! outline / breadcrumb panel: one entry per top-level Fossil definition
//! (prefix decl, mapping, function, shape ref). v0.1 emits a **flat** list (no
//! nesting) — adequate for the playground outline; per-mapping property children
//! are a v2 refinement.
//!
//! # Source: the 06-03 [`SymbolIndex`], translated to LSP coordinates
//!
//! The heavy lifting (the CST walk that finds named definitions + their byte
//! ranges) already lives in [`crate::SymbolIndex`]
//! (FILE-keyed, no per-mapping Salsa key — Research Pitfall #3). This module is a
//! thin adapter: it builds that index, maps each Fossil [`crate::SymbolKind`]
//! to an LSP [`lsp_types::SymbolKind`], and converts each byte range to a UTF-16
//! [`lsp_types::Range`] via the 06-05 [`LineIndex`] (Research Pitfall #4 — Monaco
//! counts UTF-16, so a byte range would mis-highlight after a multi-byte char).
//!
//! # Kind mapping (Research §Outline)
//!
//! | Fossil           | LSP `SymbolKind` | rationale                          |
//! |------------------|------------------|------------------------------------|
//! | `Prefix`         | `NAMESPACE`      | a `prefix ex: <iri>` is a namespace |
//! | `Mapping`        | `CLASS`          | a mapping produces typed subjects   |
//! | `Shape`          | `INTERFACE`      | a `ShEx` shape is a structural type |
//!
//! WASM-clean: returns `lsp_types::DocumentSymbol` directly (06-01 Spike A:
//! lsp-types is wasm32-clean), so the playground consumes it with no translation.

use crate::{SymbolEntry, SymbolIndex, SymbolKind as FossilSymbolKind};
use fossil_base::SourceFile;
use lsp_types::{DocumentSymbol, Position, Range, SymbolKind as LspSymbolKind};

use crate::line_index::LineIndex;
use crate::position::line_index;

/// Build the document outline for `file`: a flat `Vec<DocumentSymbol>` in source
/// order, one entry per top-level definition (prefix / mapping / function /
/// shape).
///
/// Each symbol's `range` and `selection_range` are the (UTF-16) range of the
/// defining node — v0.1 uses the same range for both (the whole declaration is
/// also the selection target). Returns an empty `Vec` for a file with no
/// top-level definitions.
#[must_use]
pub fn document_symbols(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<DocumentSymbol> {
    let index = SymbolIndex::build(db, file);
    let li = line_index(db, file);
    index
        .entries()
        .iter()
        .map(|entry| to_document_symbol(entry, &li))
        .collect()
}

/// Convert one [`SymbolEntry`] (Fossil byte range) to an LSP [`DocumentSymbol`]
/// (UTF-16 range).
fn to_document_symbol(entry: &SymbolEntry, li: &LineIndex) -> DocumentSymbol {
    let range = byte_range_to_lsp(entry.range.start, entry.range.end, li);
    #[allow(deprecated)] // `deprecated` field is required by the lsp-types struct literal
    DocumentSymbol {
        name: entry.name.to_string(),
        detail: None,
        kind: map_kind(entry.kind),
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}

/// Map a Fossil [`FossilSymbolKind`] to its LSP [`LspSymbolKind`] (see the
/// module table).
const fn map_kind(kind: FossilSymbolKind) -> LspSymbolKind {
    match kind {
        FossilSymbolKind::Mapping => LspSymbolKind::CLASS,
        FossilSymbolKind::Shape => LspSymbolKind::INTERFACE,
    }
}

/// Convert a UTF-8 byte range to a UTF-16 LSP [`Range`] via the [`LineIndex`].
fn byte_range_to_lsp(start: u32, end: u32, li: &LineIndex) -> Range {
    let s = li.position(start);
    let e = li.position(end);
    Range {
        start: Position {
            line: s.line,
            character: s.character,
        },
        end: Position {
            line: e.line,
            character: e.character,
        },
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db_file(src: &str) -> (fossil_base::FossilDb, SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    const FIXTURE: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")

User : ex:Person from users
    ex:name = .name
";

    #[test]
    fn outline_has_prefix_mapping_and_shape() {
        let (db, file) = db_file(FIXTURE);
        let syms = document_symbols(&db, file);
        assert!(
            syms.iter()
                .any(|s| s.kind == LspSymbolKind::NAMESPACE && s.name == "ex"),
            "expected a NAMESPACE for the `ex` prefix: {syms:?}"
        );
        assert!(
            syms.iter()
                .any(|s| s.kind == LspSymbolKind::CLASS && s.name == "User"),
            "expected a CLASS for the `User` mapping: {syms:?}"
        );
        assert!(
            syms.iter()
                .any(|s| s.kind == LspSymbolKind::INTERFACE && s.name == "ex:Person"),
            "expected an INTERFACE for the `ex:Person` shape ref: {syms:?}"
        );
    }

    #[test]
    fn ranges_are_nonempty() {
        let (db, file) = db_file(FIXTURE);
        for s in document_symbols(&db, file) {
            let nonempty = s.range.end.line > s.range.start.line
                || s.range.end.character > s.range.start.character;
            assert!(nonempty, "empty range for {}", s.name);
            assert_eq!(s.range, s.selection_range, "v0.1 ranges coincide");
        }
    }

    #[test]
    fn empty_file_has_no_symbols() {
        let (db, file) = db_file("");
        assert!(document_symbols(&db, file).is_empty());
    }
}
