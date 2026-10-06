//! LSP position → byte offset → [`SyntaxToken`] / [`SyntaxNode`] resolution.
//!
//! The bridge the hover handler needs to walk
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
//! # UTF-16 positions
//!
//! LSP positions count UTF-16 code units; rowan counts UTF-8 bytes. The table
//! between them is rust-analyzer's [`LineIndex`] (the `line-index` crate), and
//! the four functions below are the only way across: [`offset`] and
//! [`position`] for a point, [`range`] and [`span`] for an extent.

use ::line_index::{TextSize, WideEncoding, WideLineCol};
use fossil_base::SourceFile;
use fossil_graph_schema::Span;
use fossil_syntax::{SyntaxNode, SyntaxToken};
use lsp_types::{Position, Range};

pub use ::line_index::LineIndex;

/// The [`LineIndex`] of `file`, memoised per text.
#[salsa::tracked(returns(ref))]
pub fn line_index(db: &dyn fossil_base::Db, file: SourceFile) -> LineIndex {
    LineIndex::new(file.text(db))
}

/// An LSP position as a byte offset. `None` when the line is past the end of
/// the file; a column past the end of ITS line clamps to that line's last
/// content byte (its `\n` excluded), so the offset is always inside the text —
/// `rowan`'s `token_at_offset` panics on one that is not. A column inside a
/// surrogate pair lands on the character's start.
#[must_use]
pub fn offset(index: &LineIndex, at: Position) -> Option<u32> {
    let line = index.line(at.line)?;
    let content_end = index
        .line(at.line + 1)
        .map_or(index.len(), |next| next.start() - TextSize::from(1));
    let utf8 = index.to_utf8(
        WideEncoding::Utf16,
        WideLineCol {
            line: at.line,
            col: at.character,
        },
    )?;
    let offset = line.start() + TextSize::from(utf8.col);
    Some(offset.min(content_end).into())
}

/// A byte offset as an LSP position. An offset past the end of the file clamps
/// to its end.
#[must_use]
pub fn position(index: &LineIndex, offset: u32) -> Position {
    let utf8 = index.line_col(TextSize::from(offset).min(index.len()));
    let wide = index
        .to_wide(WideEncoding::Utf16, utf8)
        .unwrap_or(WideLineCol {
            line: utf8.line,
            col: utf8.col,
        });
    Position::new(wide.line, wide.col)
}

/// A byte span as an LSP range.
#[must_use]
pub fn range(index: &LineIndex, span: impl Into<Span>) -> Range {
    let span = span.into();
    Range::new(position(index, span.start), position(index, span.end))
}

/// An LSP range as a byte span; an end past the file clamps to its end.
#[must_use]
pub fn span(index: &LineIndex, range: Range) -> Span {
    let start = offset(index, range.start).unwrap_or_else(|| index.len().into());
    let end = offset(index, range.end).unwrap_or_else(|| index.len().into());
    Span::new(start.min(end), start.max(end))
}

/// Resolve an LSP position to the [`SyntaxToken`] that "contains" it.
///
/// Walks the CST root via [`SyntaxNode::token_at_offset`]; if the cursor is
/// on a token boundary returns the left-hand token (typical hover semantic
/// — "the token I am on / just past"). `None` when `line` is past the last
/// line or the file is empty.
///
/// A `character` past the end of its line resolves to the token at the line's
/// end rather than to `None`, because that is what [`offset`]
/// clamps it to. It used to hand `token_at_offset` an offset outside the CST,
/// which is a `rowan` panic and so a crash in whatever embeds this crate.
pub fn token_at_position(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<SyntaxToken> {
    let offset = offset(line_index(db, file), Position::new(line, character))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db_with_text(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    #[test]
    fn a_position_and_its_offset_round_trip() {
        // "abc\ndefg\nh": line 1 starts at byte 4; UTF-16 col 1 → byte 5.
        let idx = LineIndex::new("abc\ndefg\nh");
        assert_eq!(offset(&idx, Position::new(1, 1)), Some(5));
        assert_eq!(position(&idx, 5), Position::new(1, 1));
        assert!(offset(&idx, Position::new(99, 0)).is_none());
    }

    /// `é` is two bytes and one UTF-16 unit, `😀` four bytes and two units.
    #[test]
    fn a_column_counts_utf16_units() {
        let idx = LineIndex::new("é😀x\n");
        assert_eq!(offset(&idx, Position::new(0, 3)), Some(6));
        assert_eq!(position(&idx, 6), Position::new(0, 3));
        assert_eq!(position(&idx, 2), Position::new(0, 1));
    }

    /// The reproducer, verbatim: `tests/completion_property_key.rs`'s `BLANK`
    /// fixture with the indent a formatter strips. Line 4 is empty, the file is
    /// 124 bytes, and `(4, 4)` is four columns past the end of that line.
    const PAST_EOL: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")
Users : Person from User
    email = User.email

";

    /// **A column past the end of its line stays on that line.** `(4, 4)` on a
    /// blank line 4 answers the line's end, not `line_start + 4`.
    ///
    /// Before: `Some(127)` for a 124-byte file — an offset outside the text,
    /// handed to `rowan` by the caller below.
    #[test]
    fn a_column_past_the_end_of_its_line_clamps_to_it() {
        let (db, file) = db_with_text(PAST_EOL);
        let idx = line_index(&db, file);
        assert_eq!(PAST_EOL.len(), 124, "the fixture the panic was recorded on");
        // Line 4 is empty and starts at 123, the last byte being its `\n`.
        assert_eq!(offset(idx, Position::new(4, 0)), Some(123));
        assert_eq!(
            offset(idx, Position::new(4, 4)),
            Some(123),
            "a column past the line's end clamps to the line, never past it",
        );
        // The line is still the answer, not the document: a huge column on an
        // EARLIER line stops at that line's last content byte (`\n` excluded),
        // rather than running on to EOF.
        assert_eq!(
            offset(idx, Position::new(2, 9_999)),
            Some(99),
            "line 2 is `Users : Person from User`, 24 bytes from 75",
        );
        // And the line itself is still a `None`.
        assert!(offset(idx, Position::new(99, 0)).is_none());
    }

    /// **The panic.** `token_at_position` handed `rowan` the unclamped offset
    /// and `SyntaxNode::token_at_offset` aborted the process.
    ///
    /// Before: `Bad offset: range 0..124 offset 127`.
    ///
    /// No conforming editor sends this — a cursor cannot be past EOL — but
    /// every harness in this tree computes `(line, character)` by hand, the
    /// browser worker takes positions from JS, and `fossil-ide` is a library:
    /// a panic here is a crash in whatever embeds it.
    #[test]
    fn token_at_position_past_the_end_of_a_line_does_not_panic() {
        let (db, file) = db_with_text(PAST_EOL);
        let tok = token_at_position(&db, file, 4, 4);
        assert!(
            tok.is_some(),
            "the clamped offset is inside the CST, so a token resolves",
        );
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
        // We don't fix the exact kind here (the parser's node shape is free to
        // change); just confirm we got somewhere up the tree.
        assert!(!node.text().to_string().is_empty());
    }
}
