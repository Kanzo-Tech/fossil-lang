//! `fossil-ide` — hover, completion, goto-def, code actions for IDE features.
//!
//! [`Analysis::diagnostics`] is still a stub returning an empty `Vec`. The
//! crate exists so both `fossil-lsp` (the native server) and `fossil-wasm` (the
//! browser playground) can `use fossil_ide::Analysis;` without a churn commit
//! when feature work lands.
//!
//! The hover bridge:
//!
//! - [`position`]: LSP `(line, character)` → byte offset → `SyntaxToken` /
//!   `SyntaxNode`. Memoised line-offset table via Salsa-tracked
//!   [`position::line_offsets`].
//! - [`hover()`]: walks position → enclosing PROPERTY → enclosing MAPPING →
//!   `MappingLoc` (filter-then-nth) → `ExprId` →
//!   [`fossil_hir::provenance::ty_origin`] → Markdown. Destructures
//!   `ExprTypeEntry`, which is a named struct and not a tuple.
//!
//! The rest of the surface:
//!   - bidirectional hover (source-side + target-side types)
//!   - goto-def (prefixes, mappings, functions, shape refs cross-file)
//!   - completion (stdlib + prefixes + shape properties; gleam-lsp
//!     auto-import pattern)
//!   - code actions: did-you-mean Levenshtein, auto-import prefix,
//!     split-mapping suggestion
//!   - semantic tokens (Monaco depends on this)
//!   - document outline (textDocument/documentSymbol)
//!
//! ## The search layer ([`symbol_index`], [`workspace`])
//!
//! Plain-struct indexes built by a CST walk, and **no Salsa query of their
//! own** — so the per-mapping `body()` fan-out stays at 1:
//!   - [`SymbolIndex`] — per-file table of `{mapping, shape}` definitions with
//!     byte ranges (outline + goto-def hit resolution). It indexed prefix
//!     declarations too, until there were none.
//!   - [`WorkspaceIndex`] — cross-file aggregation under the
//!     open-files-as-workspace model, so a mapping or shape declared in file A
//!     resolves from file B.
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
pub mod related;
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
// `documents_named` and `registry_key` were re-exported here. They are
// `fossil_hir::documents`'s now — the compiler's own answer to which documents
// a program names and what key each is looked up under — and a re-export would
// be a second name for one function.
pub use related::{Related, related_locations};
pub use shape_documents::register_missing_documents;
pub use symbol_index::{SymbolEntry, SymbolIndex, SymbolKind};
pub use workspace::WorkspaceIndex;

/// IDE analysis entry point.
///
/// [`Self::diagnostics`] is the only method. `hover`, `completions`,
/// `goto_definition`, `code_actions`, `semantic_tokens` and `document_symbols`
/// are free functions rather than methods, because the Salsa db is passed in
/// directly — the rust-analyzer pattern.
#[derive(Debug, Default)]
pub struct Analysis;

impl Analysis {
    /// A stub: returns an empty diagnostics vector.
    ///
    /// What it owes is the drain of the Salsa `Diagnostic` accumulator after
    /// running `parse → def_map → typecheck` over `file`. `fossil-lsp` does
    /// that drain itself today, in its own `diagnostics_for`.
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
    fn diagnostics_stub_is_empty() {
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
