# fossil-playground

The browser playground for Fossil: a 5-panel editor (mapping `.fossil` /
CSVW descriptor / CSV data / ShEx shape / output graph) that runs the
compiler in a Web Worker and executes the generated SQL against
DuckDB-WASM. Replaces the Phase 0 `playground-poc/` smoke test (which
07-11 deletes).

## Status (Phase 7)

| Plan  | What it lands                                                         |
| ----- | --------------------------------------------------------------------- |
| 07-04 | This skeleton: Vite 7 + TS + 5-panel grid + baked example files       |
| 07-05 | Monaco editors + `monaco-languageclient` wiring (replaces textareas)  |
| 07-06 | DuckDB-WASM lazy load + `Run` button                                  |
| 07-07 | Mosaic / `@uwdata/vgplot` mounted in the output panel                 |
| 07-08 | Reset button + URL-based state sharing                                |
| 07-09 | Build pipeline (wasm-opt + brotli + the <10MB SC#3 budget assertion)  |
| 07-10 | Playwright e2e smoke (`npm run test:e2e`)                             |
| 07-11 | `playground-poc/` deletion + final docs polish                        |

At 07-04 the panels are placeholder `<textarea>`s. Real Monaco lands in 07-05.

## Prerequisites

- Node 20+ (developed on 23.x).
- For the WASM build glue (consumed by the playground at runtime —
  wired by 07-09): `wasm-bindgen-cli` `=0.2.120`, exact-pinned to the
  `wasm-bindgen` crate pin in the workspace `Cargo.toml` (per
  Pitfall 2 in 07-RESEARCH.md):

  ```bash
  cargo install wasm-bindgen-cli --version 0.2.120 --locked
  ```

  NB: `wasm-pack` is **forbidden** by `deny.toml` — use `wasm-bindgen-cli`
  directly + Vite (per ADR-0001/CLAUDE.md anti-pattern).

## npm scripts

| Script              | What                                                          |
| ------------------- | ------------------------------------------------------------- |
| `npm run dev`       | Vite dev server at `http://localhost:5173/` (HMR on)          |
| `npm run build`     | Production build → `dist/` with brotli `.br` sidecars         |
| `npm run preview`   | Serve a built `dist/` locally                                 |
| `npm run typecheck` | `tsc --noEmit` strict typecheck                               |
| `npm run test:e2e`  | Playwright smoke (wired in 07-10 — placeholder before that)   |

## Bootstrapping locally

```bash
cd playground/
npm install
npm run typecheck
npm run build
npm run dev   # then open http://localhost:5173/
```

## Directory layout

```
playground/
  package.json            pinned deps (see Pitfall 7 — exact-pin everything)
  vite.config.ts          brotli, ES-module workers, manualChunks for monaco
  tsconfig.json           strict ES2022 bundler resolution
  index.html              5-panel CSS grid
  src/
    main.ts               entrypoint — fetches examples + mounts panels
    panels/
      layout.css          grid scaffold (5 areas)
      mapping-panel.ts    .fossil editor (textarea → Monaco in 07-05)
      csvw-panel.ts       CSVW descriptor editor
      csv-panel.ts        CSV data editor
      shex-panel.ts       ShEx shape editor
      output-panel.ts     output graph host (Mosaic mounts here in 07-07)
  public/
    examples/             baked-in default content
      hello.fossil        Phase 1 walking skeleton
      hello.csv           Phase 1 sample data
      hello.csvw.json     minimal CSVW descriptor for hello.csv
      hello.shex          minimal ShEx shape for ex:User
```

The build pipeline orchestrator (`scripts/build-playground.sh`) lands
with 07-09 — it ties `wasm-bindgen-cli`, `wasm-opt`, and `vite build`
together and asserts the <10MB SC#3 brotli budget.
