//! LSP position → byte offset → [`SyntaxToken`] / [`SyntaxNode`] resolution.
//!
//! Phase 2 plan 02-06: the minimum bridge the hover handler needs to walk
//! from an `(line, character)` LSP position down to the enclosing
//! `MAPPING` / `PROPERTY` CST node, so it can index into the per-mapping
//! [`fossil_hir::body::HirBody`] and look up `ty_origin` in the provenance
//! side table.
//!
//! Pattern: rust-analyzer's `at_offset` over the rowan tree.
//! [`token_at_position`] uses [`rowan::TextSize`] and
//! `SyntaxNode::token_at_offset`; the `Between` case (cursor on a token
//! boundary) deterministically picks the LEFT token, matching what the LSP
//! hover semantics expect (the token the cursor "is in" / "just past").
//!
//! # UTF-16 positions (Phase 6 LSP-01)
//!
//! LSP positions use UTF-16 code units (Monaco / VS Code). Phase 2 treated the
//! `character` column as a raw byte offset — correct only for ASCII source.
//! Phase 6 plumbs proper conversion via [`crate::line_index::LineIndex`]
//! (rust-analyzer pattern, Research Pitfall #4): [`position_to_offset`] now
//! interprets `character` as a UTF-16 code unit, and [`offset_to_lsp_position`]
//! converts a byte offset back to a UTF-16 column. Both build the index from
//! the SAME FILE-keyed [`line_offsets`] table, so no new per-mapping Salsa
//! query is added (`MAX_PER_MAPPING_FAN_OUT` untouched).

use fossil_base::SourceFile;
use fossil_syntax::{SyntaxNode, SyntaxToken};

use crate::line_index::{LineIndex, Utf16Position};

/// Per-file table of byte offsets at the start of each line.
///
/// Salsa-tracked so re-derives memoise on unchanged source text. Stores a
/// `Vec<u32>` where `offsets[i]` is the byte index of the first character
/// of line `i`; `offsets[0] == 0` always.
#[salsa::tracked(debug)]
pub struct LineOffsets<'db> {
    #[returns(ref)]
    pub offsets: Vec<u32>,
}

/// Compute byte-offset-per-line table for `file`.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn line_offsets<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> LineOffsets<'db> {
    let text = file.text(db);
    let mut offsets: Vec<u32> = vec![0];
    for (i, ch) in text.bytes().enumerate() {
        if ch == b'\n' {
            // Cast is safe: source files we accept are well under u32::MAX.
            offsets.push(u32::try_from(i + 1).unwrap_or(u32::MAX));
        }
    }
    LineOffsets::new(db, offsets)
}

/// Convert an LSP `(line, character)` position to a byte offset.
///
/// `character` is interpreted as a UTF-16 code unit (the LSP wire convention),
/// converted to a UTF-8 byte offset via the [`LineIndex`]. Returns `None` if
/// `line` is past the end of the file. (Phase 6 LSP-01 — replaces Phase 2's
/// byte-offset-as-column assumption; Research Pitfall #4.)
#[must_use]
pub fn position_to_offset(index: &LineIndex, line: u32, character: u32) -> Option<u32> {
    index.offset(Utf16Position { line, character })
}

/// Convert a UTF-8 byte offset back to an LSP UTF-16 `(line, character)`.
///
/// The inverse of [`position_to_offset`]; the LSP handler uses this to
/// translate Fossil byte ranges into UTF-16-correct `lsp_types::Range`s.
#[must_use]
pub fn offset_to_lsp_position(index: &LineIndex, offset: u32) -> Utf16Position {
    index.position(offset)
}

/// Build the [`LineIndex`] for `file`.
///
/// Reads the FILE-keyed [`line_offsets`] table + the file text — a convenience
/// for the position-resolution helpers and the LSP handler. Adds no Salsa
/// query (reuses the existing memoised table).
#[must_use]
pub fn line_index(db: &dyn fossil_base::Db, file: SourceFile) -> LineIndex {
    let los = line_offsets(db, file);
    LineIndex::new(file.text(db), los.offsets(db))
}

/// Resolve an LSP position to the [`SyntaxToken`] that "contains" it.
///
/// Walks the CST root via [`SyntaxNode::token_at_offset`]; if the cursor is
/// on a token boundary returns the left-hand token (typical hover semantic
/// — "the token I am on / just past"). Returns `None` if the position is
/// past EOF or the file is empty.
pub fn token_at_position(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<SyntaxToken> {
    let index = line_index(db, file);
    let offset = position_to_offset(&index, line, character)?;
    let cst = fossil_syntax::parse(db, file);
    let root: SyntaxNode = cst.root(db).syntax();
    let offset = rowan::TextSize::new(offset);
    match root.token_at_offset(offset) {
        rowan::TokenAtOffset::None => None,
        rowan::TokenAtOffset::Single(t) => Some(t),
        rowan::TokenAtOffset::Between(l, _) => Some(l),
    }
}

/// Resolve an LSP position to the parent [`SyntaxNode`] of the token at
/// position. Convenience for hover — the type-bearing context is always a
/// parent node (PROPERTY / MAPPING / EXPR), never a leaf token.
pub fn node_at_position(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<SyntaxNode> {
    token_at_position(db, file, line, character)?.parent()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db_with_text(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    #[test]
    fn line_offsets_for_multi_line_file() {
        let (db, file) = db_with_text("abc\ndefg\nh");
        let los = line_offsets(&db, file);
        assert_eq!(los.offsets(&db), &vec![0u32, 4, 9]);
    }

    #[test]
    fn position_to_offset_handles_line_2_char_1() {
        // "abc\ndefg\nh": line 1 starts at byte 4; UTF-16 col 1 → byte 5.
        let idx = LineIndex::new("abc\ndefg\nh", &[0u32, 4, 9]);
        assert_eq!(position_to_offset(&idx, 1, 1), Some(5));
        // Inverse round-trips.
        assert_eq!(offset_to_lsp_position(&idx, 5).line, 1);
        assert_eq!(offset_to_lsp_position(&idx, 5).character, 1);
    }

    #[test]
    fn position_to_offset_returns_none_past_eof_line() {
        let idx = LineIndex::new("abc\ndefg\nh", &[0u32, 4, 9]);
        assert!(position_to_offset(&idx, 99, 0).is_none());
    }

    /// `token_at_position` on `"prefix ex: <https://example.org/>\n"` at
    /// column 8 (inside `ex`) returns an IDENT token whose text is `"ex"`.
    ///
    /// Cursor offset semantics: `Between` picks the LEFT token; column 7
    /// is exactly the boundary between the leading space and `ex`, so it
    /// would return the space. Column 8 is unambiguously inside `ex`.
    #[test]
    fn token_at_position_finds_ident() {
        let (db, file) = db_with_text("prefix ex: <https://example.org/>\n");
        let tok = token_at_position(&db, file, 0, 8).expect("expected a token at (0, 8)");
        assert_eq!(tok.text(), "ex");
    }

    /// `node_at_position` returns the parent of the token at position. For
    /// the same fixture as above, the parent of the `ex` IDENT is the
    /// `PREFIX_DECL` (or a wrapping subnode) — we just assert it is Some.
    #[test]
    fn node_at_position_returns_some_for_valid_position() {
        let (db, file) = db_with_text("prefix ex: <https://example.org/>\n");
        let node = node_at_position(&db, file, 0, 7).expect("expected a node at (0, 7)");
        // We don't fix the exact kind here (parser shape may vary across
        // Phase 2 plans); just confirm we got somewhere up the tree.
        assert!(!node.text().to_string().is_empty());
    }
}
