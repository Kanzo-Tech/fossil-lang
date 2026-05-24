# @fossil-lang/codemirror-fossil

CodeMirror 6 language extension for Fossil. Syntactic highlighting +
`@`-prefixed connector autocomplete. Standalone-importable — does NOT require
`@fossil-lang/playground`, React, DuckDB-WASM, or any LSP machinery.

## Why a separate package

Per ADR-0028 (Playground as React Library) + ADR-0030 (WASM-Exported
Tokenizer), the editor language extension is its own publishable package
so consumers (e.g. Keasy's existing CodeMirror editor) can migrate off
their hand-rolled `StreamLanguage` lexer onto the single grammar source of
truth without pulling in the entire React playground bundle.

## Install

```bash
pnpm add @fossil-lang/codemirror-fossil @fossil-lang/wasm \
         @codemirror/state @codemirror/view @codemirror/language \
         @codemirror/autocomplete @codemirror/lint
```

All `@codemirror/*` packages are `peerDependencies` to avoid the duplicate
CodeMirror state crash (RESEARCH.md Pitfall 10 — `@codemirror/state` keeps
private identity-comparison maps that crash if two copies coexist in the
dep tree). `@fossil-lang/wasm` is also a peer (the host installs it once,
shares it across editor + LSP + run path).

## Use

```typescript
import { EditorView } from '@codemirror/view';
import { EditorState } from '@codemirror/state';
import { fossil } from '@fossil-lang/codemirror-fossil';
import { initFossilWasm } from '@fossil-lang/wasm';

await initFossilWasm({ wasmUrl: '/path/to/fossil_wasm_bg.wasm' });

new EditorView({
  state: EditorState.create({
    doc: 'prefix ex: <https://example.org/>',
    extensions: [
      // Optional resolver — drives `@`-autocomplete. Omit to disable.
      fossil({ resolver: myConnectionResolver }),
    ],
  }),
  parent: document.body,
});
```

### What `fossil()` includes (v0.1)

- **Syntactic highlighting** via `@fossil-lang/wasm` `tokenize()` (the
  canonical Rust lexer, ADR-0030). No TS-side lexer reimplementation.
- **`@`-prefixed autocomplete** sourced from `resolver.list()` (the
  ADR-0029 IoC contract). Stage 1 — connector-name completion. Stage 2
  (per-connector path completion) returns null in v0.1; deferred until
  resolvers grow a `listPaths()` capability.

### What `fossil()` does NOT include

- **LSP diagnostics, hover, goto-definition, semantic-tokens overlay,
  completion-from-LSP** — these are wired in 08-09's
  `@fossil-lang/playground` via `@codemirror/lsp-client` (ADR-0032).
  This package provides the language foundation; the LSP integration
  layers on top via the playground composition.
- **React** — this is a pure CodeMirror extension. The React wrapper is in
  `@fossil-lang/playground`.

## Architectural notes

- `fossilStreamParser` re-tokenizes the full document on each first-of-line
  call after a change — O(n) per change for n-byte input. For Fossil's
  typical <500 LOC mappings this is <2ms; acceptable for v0.1. Switching
  to a true Lezer parser with incremental re-parsing is out of scope for
  Phase 8.
- `FossilKind` mirrors `fossil_syntax::lexer::Token` declaration order.
  Per ADR-0030 § Negative consequences, reordering Rust `Token` variants
  is a BREAKING CHANGE for this package — bump major in lockstep.
  Appending new variants is backwards-compatible (unknown kinds fall
  through to `null`, no crash).
- `Ident` tokens map to `null` (no opinionated syntactic highlight) on
  purpose: the LSP semantic-tokens overlay in 08-09 paints these as
  `variable`/`function`/`type`/`property` based on HIR resolution. The
  syntactic layer doesn't have enough information to do better.

## ADR cross-references

- [ADR-0027 — CodeMirror over Monaco](../../decisions/0027-codemirror-over-monaco.md)
- [ADR-0028 — Playground as React Library](../../decisions/0028-playground-as-react-library.md)
- [ADR-0029 — Two-Tier Source Resolution](../../decisions/0029-two-tier-source-resolution.md)
- [ADR-0030 — WASM-Exported Tokenizer](../../decisions/0030-wasm-exported-tokenizer.md)
- [ADR-0032 — LSP Client Choice](../../decisions/0032-lsp-client-choice.md)

## License

Apache-2.0
