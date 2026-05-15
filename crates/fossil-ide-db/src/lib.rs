//! `fossil-ide-db` — symbol indexes + search infrastructure for IDE features.
//!
//! Phase 1: empty placeholder. The crate exists so Phase 6 LSP-01 has a slot
//! to fill in without churning the workspace member list.
//!
//! Phase 6 LSP-01 adds:
//!   - `SymbolIndex` (per file, query: "find all references to symbol X")
//!   - `PrefixIndex` (cross-file prefix completion for the gleam-lsp
//!     auto-import pattern)
//!   - `ShapeIndex` (cross-file shape resolution for goto-def on shape refs)
//!
//! Pattern: rust-analyzer's `ide-db` split from `ide` (search infrastructure
//! reusable across consumers — both `fossil-ide` for in-process feature
//! dispatch and `fossil-lsp` for the LSP server).

#![allow(unused)]
