---
"@fossil-lang/codemirror-fossil": patch
"@fossil-lang/wasm": patch
"@fossil-lang/types": patch
"@fossil-lang/resolvers": patch
"@fossil-lang/examples": patch
---

_Bump-level downgraded from `minor` to `patch` as part of Phase 17 REL-01 release squash (this changeset's narrative is preserved; the v0.2.0 minor bump is carried by `release-v0-2-0.md`)._

Initial public release of the Fossil playground React library family.

- `@fossil-lang/codemirror-fossil` — CodeMirror 6 language extension delegating tokenization to the canonical Rust lexer via the `tokenize()` export from `@fossil-lang/wasm` (ADR-0030 single grammar source of truth). Importable standalone for hosts that want only the editor (Keasy migration target).
- `@fossil-lang/wasm` — JS/TS wrapper around the `fossil-wasm` wasm-bindgen artifacts (`--target web` per RESEARCH.md Pitfall 1). Exposes `initFossilWasm({ wasmUrl })`, `tokenize`, `semanticLegend`, and the `FossilPlayground` class.
- `@fossil-lang/types` — shared TypeScript types (zero runtime): `SourceRef`, `ResolvedSource`, `Connector`, `ConnectionResolver`, `FossilTheme`, `TokenRow`, `SemanticTokensLegend`, `Diagnostic`.
- `@fossil-lang/resolvers` — default + mock + public-HTTP `ConnectionResolver` implementations (Tier 1 per ADR-0029). Host injects this prop; the component never sees plaintext credentials (CONN-01 invariant; greppable test).
- `@fossil-lang/examples` — bundled `hello.fossil` + `.csv` + `.csvw.json` + `.shex` fixtures using `@examples/...` paths per CONN-03.

See `.planning/phases/08-playground-react-library-v0-1/SUMMARY.md` for the full phase close + 5 success criteria evidence.

Architecture: ADRs 0024 (Workspace API), 0026 (Two Workers Two Lifecycles), 0027 (CodeMirror over Monaco), 0028 (React library distribution), 0029 (two-tier source resolution), 0030 (WASM-exported tokenizer), 0031 (pnpm monorepo), 0032 (LSP client choice).
