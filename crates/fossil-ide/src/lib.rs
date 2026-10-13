//! `fossil-ide` — hover, completion, goto-def, code actions for IDE features.
//!
//! The crate answers each editor question once, transport-free, so the host
//! (`fossil-wasm`, the browser worker) only translates. It owns no answer:
//! every feature asks the semantic layer in `fossil-hir`, as `ty_ide` asks
//! `ty_python_semantic`'s `SemanticModel` and rust-analyzer's `ide` asks `hir`.
//!
//! - **Names** resolve through ONE resolver, [`fossil_hir::def_map::DefMap::resolve`],
//!   which is what lowering and the checker read. [`mod@defs`] classifies the
//!   token under the cursor by its node and asks it; goto-definition and the
//!   outline read the [`fossil_hir::def_map::Definition`] it answers, so a new
//!   kind of definition is a variant there and not a branch per feature.
//! - **Types** come from the checker: hover reads
//!   [`fossil_hir::provenance::ty_origin`], completion the resolved target
//!   shape and the source row, goto-definition the resolved type's own file.
//! - **Positions** are [`mod@position`]: LSP `(line, character)` → byte offset →
//!   `SyntaxToken` / `SyntaxNode`, over rust-analyzer's `line-index`, memoised
//!   per file by [`position::line_index`].
//!
//! The surface:
//!   - hover, completion, goto-definition, related locations
//!   - code actions: did-you-mean Levenshtein, split-mapping suggestion
//!   - semantic tokens (the browser editor paints from them)
//!   - document outline (textDocument/documentSymbol)
//!   - diagnostics, as the LSP shape and the wasm one
//!
//! Nothing here adds a query keyed by a mapping, so the per-mapping `body()`
//! fan-out `fossil-hir/tests/invalidation_regression.rs` counts is the
//! checker's alone.

pub mod code_action;
pub mod completion;
pub mod constrain;
pub mod defs;
pub mod diagnostics;
pub mod goto_def;
pub mod hover;
pub mod outline;
pub mod position;
pub mod related;
pub mod semantic;
pub mod wire;

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

// There is no `Analysis` namespace type: the db is passed in directly, so
// everything in this crate is a free function.
