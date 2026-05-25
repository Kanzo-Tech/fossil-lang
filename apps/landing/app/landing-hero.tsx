/**
 * Landing hero — top-of-page content above the playground (PLAY-06).
 *
 * Server Component (no `'use client'`, no hooks). The hero is pure
 * presentation — typing into the CTA scrolls to the `#playground`
 * anchor via the browser's native hash-anchor behaviour; no JS needed.
 *
 * Contents (per 09-CONTEXT.md PLAY-06 decisions + 09-08 plan §1):
 *   - Tagline + value proposition.
 *   - "10-second example" CTA → `#playground` anchor on the same page.
 *   - Pipeline diagram (3 boxes: `.fossil` → DuckDB SQL → GraphAr Parquet).
 *   - GitHub repo link.
 *   - Min Oo & Hartig ESWC 2025 paper link (arXiv 2503.10385) — the
 *     foundational citation per the orchestrator's downstream_consumer
 *     block.
 *   - BibTeX-friendly cite snippet for the foundational paper, inside a
 *     `<details>` so it stays out of the way for casual visitors.
 *
 * Bundle hygiene — the `HARTIG_BIBTEX` constant is INLINED here rather
 * than imported from `@fossil-lang/playground`. We need this constant in
 * a Server Component, but the playground barrel re-exports client-only
 * components (FossilEditor, ResultGraph) that use `useRef`/`useEffect`.
 * Next.js 15's Server Component static analysis follows the import graph
 * and refuses to compile a Server Component that transitively imports a
 * hook — even if the importing file only uses a string constant.
 *
 * The plan's §1 note anticipated this: *"If a build-time tree-shake
 * issue arises, copy the string verbatim into `landing-hero.tsx`."*
 * Copy lives below; the parallel `landing-bibtex.spec.ts` E2E asserts
 * its content matches `@fossil-lang/playground`'s `HARTIG_BIBTEX` byte-
 * for-byte so they cannot drift.
 *
 * a11y: inherits WCAG 2.1 AA from Phase 8 (axe-core gate in
 * `apps/landing/tests/e2e/a11y.spec.ts` runs against this hero too).
 *   - <h1> opens the document hierarchy; the playground's role="application"
 *     follows it.
 *   - `aria-label="Project introduction"` on the outer <section> gives
 *     SR users a landmark name.
 *   - The diagram's row of boxes carries `aria-label="Compilation pipeline"`
 *     so the visual flow has an accessible name; the connecting arrows are
 *     marked `aria-hidden` because they're decorative.
 *   - The links carry `rel="noopener noreferrer"` per WCAG-adjacent
 *     security hardening (cross-origin links + `target="_blank"` opens).
 */

/**
 * Verbatim copy of `@fossil-lang/playground`'s `HARTIG_BIBTEX`. Kept in
 * sync via `landing-hero.spec.ts` which asserts the rendered <pre> matches
 * the cite-templates module's value. Edits MUST be mirrored in
 * `packages/playground/src/bibtex/cite-templates.ts`.
 */
const HARTIG_BIBTEX_INLINE = `@inproceedings{minoo-hartig-2025-algebraic,
  title     = {An Algebraic Foundation for Knowledge Graph Construction},
  author    = {Min Oo, Sitt and Hartig, Olaf},
  booktitle = {Proceedings of the 22nd European Semantic Web Conference (ESWC 2025)},
  year      = {2025},
  note      = {Best Research Paper Award},
  url       = {https://arxiv.org/abs/2503.10385},
}`;

export function LandingHero(): JSX.Element {
  return (
    <section className="landing-hero" aria-label="Project introduction">
      <h1>Fossil — typed mapping DSL for knowledge graphs</h1>
      <p className="landing-tagline">
        Type-checked mappings, instant browser feedback.{' '}
        <strong>If it compiles, the graph is well-formed.</strong>
      </p>
      <a
        href="#playground"
        className="landing-cta"
        data-testid="landing-cta"
      >
        Try the 10-second example
      </a>

      <div className="landing-diagram" aria-label="Compilation pipeline">
        <div className="landing-diagram-box">.fossil</div>
        <span aria-hidden="true" className="landing-diagram-arrow">
          →
        </span>
        <div className="landing-diagram-box">DuckDB SQL</div>
        <span aria-hidden="true" className="landing-diagram-arrow">
          →
        </span>
        <div className="landing-diagram-box">GraphAr / Parquet</div>
      </div>

      <nav className="landing-links" aria-label="Project links">
        <a
          href="https://github.com/kanzo-tech/fossil"
          data-testid="landing-repo-link"
          rel="noopener noreferrer"
        >
          GitHub repository
        </a>
        <a
          href="https://arxiv.org/abs/2503.10385"
          data-testid="landing-paper-link"
          rel="noopener noreferrer"
        >
          Foundations: Min Oo &amp; Hartig (ESWC 2025)
        </a>
      </nav>

      <details className="landing-cite">
        <summary>Cite the foundational paper (BibTeX)</summary>
        <pre data-testid="landing-bibtex" className="landing-bibtex-block">
          <code>{HARTIG_BIBTEX_INLINE}</code>
        </pre>
      </details>
    </section>
  );
}
