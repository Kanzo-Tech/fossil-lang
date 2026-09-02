//! `fossil-ide` — hover, completion, goto-def, code actions for IDE features.
//!
//! The crate exists so both editor hosts — `fossil-lsp` (the native server) and
//! `fossil-wasm` (the browser worker) — answer a question once. Each is a
//! transport with a host attached; neither owns an answer.
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
//!   - goto-def (shape names and property keys into the shape document;
//!     mappings and `:=` bindings across the open files)
//!   - completion (stdlib + shape properties + source fields)
//!   - code actions: did-you-mean Levenshtein, split-mapping suggestion
//!   - semantic tokens (Monaco depends on this)
//!   - document outline (textDocument/documentSymbol)
//!
//! ## The search layer ([`symbol_index`], [`workspace`])
//!
//! Plain-struct indexes built by a CST walk, and **no Salsa query of their
//! own** — so the per-mapping `body()` fan-out stays at 1:
//!   - [`SymbolIndex`] — per-file table of `{source, mapping, shape}`
//!     definitions with byte ranges (outline + goto-def hit resolution).
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
//! They were `fossil-ide-db`, split off on the strength of rust-analyzer's
//! `ide-db`/`ide` split, until this crate absorbed them: a crate boundary that
//! separates nothing is a file boundary.

pub mod code_action;
pub mod completion;
pub mod diagnostics;
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
pub use diagnostics::{
    byte_range_to_range, diagnostics, file_uri, lsp_diagnostic, lsp_diagnostics, span_to_range,
};
pub use goto_def::{NavigationTarget, goto_definition};
pub use hover::{HoverInfo, hover, hover_bidirectional};
pub use line_index::{LineIndex, Utf16Position};
pub use outline::document_symbols;
pub use position::{
    LineOffsets, line_index, line_offsets, node_at_position, offset_to_lsp_position,
    position_to_offset, token_at_position,
};
pub use semantic::{decode_tokens, legend_type_name, semantic_legend, semantic_tokens};
pub use related::{Related, related_locations};
pub use shape_documents::register_missing_documents;
pub use symbol_index::{SymbolEntry, SymbolIndex, SymbolKind};
pub use workspace::WorkspaceIndex;

// There is no `Analysis` namespace type: the db is passed in directly, so
// everything in this crate is a free function.
