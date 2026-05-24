# ADR 0030: Export Fossil tokenizer from `fossil-wasm` (single grammar source of truth)

**Date:** 2026-05-24
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:**
- `.planning/research/playground-sources-design.md` §4 "Lexer"
- `decisions/0001-use-lsp-server-not-tower-lsp.md` (the `one crate, two hosts` framing this extends)
- ADR-0024 (`fossil-wasm` Workspace API — the same crate this extends with `tokenize`)
- ADR-0027 (CodeMirror 6 — the consumer that needs tokens for highlighting)
- `crates/fossil-syntax/src/lexer.rs` (logos 0.16 — the canonical lexer)
- Phase 6 plan 06-07 (LSP `semanticTokens` — pre-existing semantic-highlighting path that layers atop syntactic)

## Context

The conversation surfaced a question worth recording: if the Fossil
compiler already owns a canonical lexer written in Rust (`crates/fossil-
syntax/src/lexer.rs`, logos 0.16), why would the editor maintain a
parallel JS/TS lexer for syntax highlighting?

Keasy currently does this — its `web/src/components/discovery/code-editor.tsx`
ships a CodeMirror `StreamLanguage` that reimplements Fossil token
recognition in TypeScript. Any grammar evolution in the Rust lexer must be
ported to the TS lexer for the editor to stay correct. Drift is inevitable;
correctness depends on dual maintenance forever.

The ADR-0024 pattern (one Rust crate, multiple hosts) generalises to the
lexer: one tokenization function, multiple consumers (compiler, LSP,
editor across every host).

Browser-side tokenization performance is the standard concern when
exporting WASM functions for editor highlighting. For Fossil:
- A typical mapping is < 500 LOC; full re-tokenize on each keystroke
  is sub-millisecond in benchmarks of comparable WASM-exposed lexers
  (e.g., Astral's `ty_wasm`).
- For larger files (~5000 LOC), incremental tokenization can be added
  later. Out of scope for v0.1.
- The CodeMirror `StreamParser` interface expects per-token streaming;
  the WASM bridge can return tokens lazily (return an iterator handle)
  or eagerly (return a token array); we will start eager and revisit
  only if profiling shows pressure.

Semantic highlighting (type-aware) is orthogonal — Phase 6 plan 06-07
already implements LSP `textDocument/semanticTokens/full` returning a
flat 5-tuple delta stream. CodeMirror consumes this via the same
`codemirror-languageserver` bridge as diagnostics. The syntactic tokenizer
(this ADR) is the bootstrap path; the semantic tokens enrich on top
as soon as the LSP first response lands.

## Decision

`fossil-wasm` grows a `tokenize` export:

```rust
#[wasm_bindgen]
pub fn tokenize(text: &str) -> JsValue {
    let tokens: Vec<TokenRow> = fossil_syntax::lex(text)
        .map(|(kind, range)| TokenRow {
            kind: kind as u32,
            start: range.start as u32,
            end: range.end as u32,
        })
        .collect();
    serde_wasm_bindgen::to_value(&tokens).unwrap()
}

pub fn tokenize_native(text: &str) -> Vec<TokenRow> { /* same, for cargo-test */ }
```

`TokenRow` is a plain struct serialisable to JS — `SyntaxKind` is exposed
as a numeric tag (the JS side maps tag → CodeMirror highlight category via
a single lookup table in `@fossil-lang/codemirror-fossil`).

`@fossil-lang/codemirror-fossil` provides a CodeMirror `StreamParser` that
calls `tokenize(text)` (re-tokenize per change for v0.1) and maps tokens
to highlight tags. The package re-exports the kind enum and the tag map
so consumers can theme tokens individually.

The Rust lexer in `crates/fossil-syntax/` remains the canonical Fossil
grammar. NO JS/TS-side reimplementation. Keasy migrates to consume
`@fossil-lang/codemirror-fossil` (which depends on `@fossil-lang/wasm`)
and deletes its internal StreamLanguage.

## Consequences

**Positive.**

- **Single grammar source of truth across the entire org.** Compiler,
  LSP, native editor (VS Code via Phase 9 extension), browser editor
  (every consumer of `@fossil-lang/codemirror-fossil`) all derive from
  one Rust lexer. Grammar evolution propagates automatically.
- ADR-0024's "one crate, two hosts" framing generalises naturally —
  the same `fossil-wasm` artifact now hosts: the Workspace API (24),
  `compile_file` + `set_target_shex` (24), and `tokenize` (this ADR).
  No new gated crate; the 9-crate WASM gate stays at 9.
- Keasy's StreamLanguage duplicate disappears, removing a chronic
  drift source.
- Bootstrap is fast: the WASM bundle is loaded eagerly by the
  playground anyway (the LSP Worker needs it on first interaction);
  syntax highlighting reuses that load with no incremental cost
  beyond per-keystroke tokenization.
- Semantic highlighting layers cleanly on top via the existing LSP
  `semanticTokens` (06-07) — no new transport, no new format.

**Negative.**

- A `@fossil-lang/codemirror-fossil` consumer must also load
  `@fossil-lang/wasm` (~500KB–1MB compressed). For a "syntax-only,
  no editor needed" consumer this is overkill; we accept the cost
  for v0.1 and defer a `fossil-wasm-lex-only` micro-bundle until a
  consumer asks (none have).
- The WASM bridge is a sync call from the editor's perspective; an
  unusually large file (~10k LOC) could measurably stutter on a
  cold tokenize. We will benchmark at Phase 8 close; if it bites,
  introduce incremental tokenization (re-tokenize only the changed
  range + a small context) or move tokenization off the main thread
  via the existing LSP Worker.
- The `TokenRow` shape is a public API across the Rust ↔ JS
  boundary — additions are backwards-compatible (new optional
  fields) but renames require coordinated package bumps.

**Neutral.**

- The semantic-tokens path (06-07) is unchanged. Syntactic tokens
  provide the bootstrap render; semantic tokens land asynchronously
  on the first LSP response and enrich highlighting in place. No
  user-visible flash for files of any plausible Fossil size.
- The lexer enum (`SyntaxKind`) gains a public-stable tag mapping
  exposed via WASM; this is documented in the Rust crate header
  and the codemirror-fossil README. Reordering enum variants becomes
  a breaking change for downstream consumers — enforced by a
  `#[non_exhaustive]` discriminant plan if the enum stabilizes.

## Alternatives considered

1. **Maintain a parallel TS lexer (the Keasy status quo).** Rejected
   — guarantees drift, doubles maintenance, every grammar PR has a
   secondary TS lexer PR.

2. **Tree-sitter Fossil grammar shipped as a separate package
   consumed by both the compiler and the editor.** Rejected —
   would replace the canonical logos-based lexer in the compiler,
   require a tree-sitter Fossil grammar to be written and maintained
   in addition to the existing parser, and provide no benefit
   `tokenize` doesn't already provide. Tree-sitter shines for
   incremental parsing; for tokenization-only we already have a
   working answer.

3. **Skip syntax highlighting; rely entirely on LSP semantic tokens.**
   Rejected — first-load latency is visible (the user sees plain
   text for the ~100ms before the LSP responds); a syntactic
   bootstrap eliminates the flash. Also: not every consumer of
   `@fossil-lang/codemirror-fossil` will mount the LSP (e.g., a
   read-only docs embed), and those consumers still want
   highlighting.

4. **Ship a separate `fossil-lexer-wasm` micro-crate.** Rejected
   for v0.1 — adds a 10th gated crate for zero immediate benefit;
   the existing `fossil-wasm` crate is the right home. Reconsider
   if a "lexer only" use case emerges.
