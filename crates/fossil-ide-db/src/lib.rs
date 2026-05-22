//! `fossil-ide-db` — symbol indexes + search infrastructure for IDE features.
//!
//! Pattern: rust-analyzer's `ide-db` split from `ide` — the WASM-clean search
//! layer reusable across consumers (`fossil-ide` for in-process feature
//! dispatch and `fossil-lsp` for the LSP server, plus the playground host).
//!
//! Phase 6 LSP-01 (plan 06-03) fills it with three layers:
//!   - [`SymbolIndex`] — per-file table of `{prefix, mapping, function, shape}`
//!     definitions with byte ranges (outline + goto-def hit resolution).
//!   - [`PrefixIndex`] — prefix → IRI resolution with a well-known fallback
//!     (the gleam-lsp auto-import completion pattern).
//!   - [`WorkspaceIndex`] — cross-file aggregation under the
//!     open-files-as-workspace model (ADR-0023), so a prefix/mapping/function/
//!     shape declared in file A resolves from file B.
//!
//! WASM invariant: this crate is in the 9-crate WASM gate (06-01). It depends
//! only on `fossil-base` + `fossil-syntax` (both WASM-clean) and adds NO Salsa
//! query of its own — the indexes are plain structs built by a CST walk, so the
//! per-mapping `body()` fan-out stays at 1 (Research Pitfall #3).

pub mod prefix_index;
pub mod symbol_index;

pub use prefix_index::{PrefixBinding, PrefixIndex, WELL_KNOWN_PREFIXES};
pub use symbol_index::{SymbolEntry, SymbolIndex, SymbolKind};
