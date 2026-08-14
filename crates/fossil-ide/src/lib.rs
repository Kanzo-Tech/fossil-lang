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
//!   `MappingLoc` (filter-then-nth) → `ExprId` →
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
//!
//! ## The search layer ([`symbol_index`], [`prefix_index`], [`workspace`])
//!
//! Three plain-struct indexes built by a CST walk, and **no Salsa query of
//! their own** — so the per-mapping `body()` fan-out stays at 1:
//!   - [`SymbolIndex`] — per-file table of `{prefix, mapping, function, shape}`
//!     definitions with byte ranges (outline + goto-def hit resolution).
//!     (the gleam-lsp auto-import completion pattern).
//!   - [`WorkspaceIndex`] — cross-file aggregation under the
//!     open-files-as-workspace model, so a prefix/mapping/function/
//!     shape declared in file A resolves from file B.
//!
//! ## The host's half ([`shape_documents`])
//!
//! Not a feature — the wiring both editor hosts need before any feature is
//! correct. The shape document a program names is a Salsa **input**, so
//! `fossil-lsp` and `fossil-wasm` have to register it before the checker asks
//! for it, and both do it from the same three functions there. See that
//! module's docs for why an editor needs this more than a batch compile does.
//!
//! They were `fossil-ide-db` until this crate absorbed them, on the strength of
//! rust-analyzer's `ide-db`/`ide` split. That split carries ~20k lines shared
//! by five crates; this one carried 592 lines with one consumer, no Salsa, no
//! macro, no `tests/`, and a dependency set that was a strict subset of this
//! crate's. A crate boundary that separates nothing is a file boundary.

pub mod code_action;
pub mod completion;
pub mod goto_def;
pub mod hover;
pub mod line_index;
pub mod outline;
pub mod position;
pub mod semantic;
pub mod shape_documents;
pub mod symbol_index;
pub mod workspace;

pub use code_action::code_actions;
pub use completion::completions;
pub use goto_def::{NavigationTarget, goto_definition};
pub use hover::{HoverInfo, hover, hover_bidirectional};
pub use line_index::{LineIndex, Utf16Position};
pub use outline::document_symbols;
pub use position::{
    LineOffsets, line_index, line_offsets, node_at_position, offset_to_lsp_position,
    position_to_offset, token_at_position,
};
pub use semantic::{decode_tokens, legend_type_name, semantic_legend, semantic_tokens};
pub use shape_documents::{documents_named, register_missing_documents, registry_key};
pub use symbol_index::{SymbolEntry, SymbolIndex, SymbolKind};
pub use workspace::WorkspaceIndex;

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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
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
