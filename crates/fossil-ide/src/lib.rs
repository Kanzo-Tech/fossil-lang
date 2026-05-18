//! `fossil-ide` — hover, completion, goto-def, code actions for IDE features.
//!
//! Phase 1: [`Analysis::diagnostics`] returns an empty `Vec`. The crate exists
//! so both `fossil-lsp` (Phase 6 server) and `fossil-wasm` (Phase 7 playground)
//! can `use fossil_ide::Analysis;` without a churn commit when feature work
//! lands.
//!
//! Phase 2 plan 02-06 (this plan) lands the minimum hover bridge:
//!
//! - [`position`]: LSP `(line, character)` → byte offset → `SyntaxToken` /
//!   `SyntaxNode`. Memoised line-offset table via Salsa-tracked
//!   [`position::line_offsets`].
//! - [`hover`]: walks position → enclosing PROPERTY → enclosing MAPPING →
//!   `MappingLoc` (per ADR-0005 filter-then-nth) → `ExprId` →
//!   [`fossil_hir::provenance::ty_origin`] → Markdown. Destructures
//!   `ExprTypeEntry` (not a tuple) per planner checker Blocker 5.
//!
//! Phase 6 LSP-01 grows the rest of the surface:
//!   - bidirectional hover (source-side + target-side types)
//!   - goto-def (prefixes, mappings, functions, shape refs cross-file)
//!   - completion (stdlib + prefixes + shape properties; gleam-lsp
//!     auto-import pattern)
//!   - code actions: did-you-mean Levenshtein, auto-import prefix,
//!     split-mapping suggestion
//!   - semantic tokens (Monaco depends on this)
//!   - document outline (textDocument/documentSymbol)

pub mod hover;
pub mod position;

pub use hover::{HoverInfo, hover};
pub use position::{
    LineOffsets, line_offsets, node_at_position, position_to_offset, token_at_position,
};

/// IDE analysis entry point.
///
/// Phase 1 shipped only [`Self::diagnostics`]; Phase 2 plan 02-06 adds the
/// free-function [`hover`] surface (not a method on `Analysis` because the
/// Salsa db is passed in directly — rust-analyzer pattern). Phase 6 LSP-01
/// grows `completion`, `goto_def`, `code_actions`, `semantic_tokens`,
/// `document_symbols`.
#[derive(Debug, Default)]
pub struct Analysis;

impl Analysis {
    /// Phase 1 stub: returns an empty diagnostics vector.
    ///
    /// Phase 6 LSP-01 drains the Salsa `Diagnostic` accumulator after running
    /// `parse → def_map → typecheck` queries on `file`.
    #[must_use]
    pub fn diagnostics(
        _db: &dyn fossil_base::Db,
        _file: fossil_base::SourceFile,
    ) -> Vec<fossil_base::Diagnostic> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn diagnostics_phase1_stub_is_empty() {
        use std::sync::Arc;
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            "prefix ex: <a>".to_string(),
            "test.fossil".to_string(),
        );
        let diags = Analysis::diagnostics(&db, file);
        assert!(diags.is_empty());
    }
}
