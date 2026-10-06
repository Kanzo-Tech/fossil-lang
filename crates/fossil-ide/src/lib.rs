//! `fossil-ide` — hover, completion, goto-def, code actions for IDE features.
//!
//! The crate exists so both editor hosts — `fossil-lsp` (the native server) and
//! `fossil-wasm` (the browser worker) — answer a question once. Each is a
//! transport with a host attached; neither owns an answer.
//!
//! The hover bridge:
//!
//! - [`mod@position`]: LSP `(line, character)` → byte offset → `SyntaxToken` /
//!   `SyntaxNode`, over rust-analyzer's `line-index`, memoised per file by
//!   [`position::line_index`].
//! - [`hover()`]: walks position → enclosing PROPERTY → enclosing MAPPING →
//!   `MappingLoc` (filter-then-nth) → `ExprId` →
//!   [`fossil_hir::provenance::ty_origin`] → Markdown. Destructures
//!   `ExprTypeEntry`, which is a named struct and not a tuple.
//!
//! The rest of the surface:
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
//! They were `fossil-ide-db`, split off on the strength of rust-analyzer's
//! `ide-db`/`ide` split, until this crate absorbed them: a crate boundary that
//! separates nothing is a file boundary.

pub mod code_action;
pub mod completion;
pub mod diagnostics;
pub mod goto_def;
pub mod hover;
pub mod outline;
pub mod position;
pub mod related;
pub mod semantic;
pub mod symbol_index;
pub mod wire;
pub mod workspace;

pub use code_action::code_actions;
pub use completion::completions;
pub use diagnostics::{
    DiagnosticData, Replacement, diagnostics, file_uri, lsp_diagnostic, lsp_diagnostics,
};
pub use goto_def::{NavigationTarget, goto_definition};
pub use hover::{HoverInfo, hover};
pub use outline::document_symbols;
pub use position::{
    LineIndex, line_index, node_at_position, offset, position, range, span, token_at_position,
};
pub use related::{Related, related_locations};
pub use semantic::{
    SemanticSpan, TokenType, decode_tokens, modifier_names, semantic_legend, semantic_spans,
    semantic_tokens,
};
pub use symbol_index::{SymbolEntry, SymbolIndex, SymbolKind};
pub use workspace::WorkspaceIndex;

// There is no `Analysis` namespace type: the db is passed in directly, so
// everything in this crate is a free function.
