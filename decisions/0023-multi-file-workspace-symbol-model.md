# ADR 0023: Model the workspace as the set of open files, not a scanned root

**Date:** 2026-05-22
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/06-cli-complete-lsp/06-RESEARCH.md` §"the multi-file model (open-files-as-workspace vs scan-workspace-root)"

## Context

Cross-file goto-def and the gleam-lsp auto-import completion (SC#4, LSP-01) need
a symbol table that spans more than one document: a prefix declared in file A
must resolve when used in file B, and completion must offer mappings/functions
defined elsewhere in the project. The `fossil-ide-db` search layer therefore
needs a notion of "the workspace" — the set of files over which the
[`WorkspaceIndex`] aggregates per-file [`SymbolIndex`]es.

Two models are in tension. **Open-files-as-workspace** treats the workspace as
exactly the files the host currently holds open: the LSP server's
`LspState.files` map (the documents the editor has sent via `didOpen`), or the
playground's multi-panel set (every panel is conceptually "open"). **Scan-
workspace-root** treats the workspace as a directory tree rooted at the LSP
`initialize` request's `workspace_folders` (currently ignored), recursively
discovering every `.fossil` file on disk.

The scan-root model is heavier: it needs filesystem traversal, which conflicts
with the WASM target. `fossil-ide-db` is in the 9-crate WASM gate (ADR/Spike
06-01) and the playground runs against a VirtualFS, not a real directory tree —
there is no root to walk in the browser. It also raises invalidation and
indexing-cost questions (when does a large tree get re-scanned?) that v0.1 does
not need to answer to ship cross-file goto-def over the handful of files a user
actually has open.

## Decision

We will model the workspace as the **set of open files** for v0.1.
`WorkspaceIndex::build(db, files: &[SourceFile])` takes the host-supplied open
set explicitly: the LSP passes its `LspState.files`, the playground passes its
panel set. The index is a plain struct (no Salsa query, no `Box<dyn>`), so it
adds zero tracked queries and does not widen the per-mapping `body()` fan-out.

A filesystem `workspace_folders` scan-root is deferred to v2. The LSP
`initialize` `workspace_folders` field (today bound to `_initialization_params`
and ignored) is the future hook: a v2 indexer can walk that root, add the
discovered files to the open set, and reuse the same `WorkspaceIndex` resolve
API unchanged.

## Consequences

Cross-file goto-def and auto-import work immediately over the open files, with
no FS dependency — so the same code path serves both the native LSP and the
WASM playground (the gate stays green). Closing a file removes its symbols from
resolution, which matches editor intuition (you cannot jump to a definition in a
file you have closed) and keeps the index bounded by what the user is actively
editing.

The cost is that a definition in an unopened project file is invisible to
goto-def until that file is opened — acceptable for v0.1's small multi-file
examples, and the documented v2 path (scan `workspace_folders`) closes the gap
without changing the resolve API. `WorkspaceIndex::resolve` returns ALL matches
across files rather than picking one, leaving ambiguity (e.g. a mapping name
re-declared in two open files) for the caller to surface rather than silently
collapsing it.
