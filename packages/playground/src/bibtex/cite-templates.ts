/**
 * Citation templates for the playground (PLAY-08 + PLAY-06).
 *
 * Two reference entries shipped:
 *
 *   1. **Min Oo & Hartig 2025** — the foundational paper Fossil's type
 *      system is built on. Cited in both the landing hero (PLAY-06) and
 *      the playground's BibTeX modal (PLAY-08) for paper drafts. The
 *      orchestrator's `downstream_consumer` block (echoed in 09-CONTEXT.md
 *      and the 09-08 plan) resolves RESEARCH.md Open Question #1 to this
 *      exact citation: ESWC 2025, Best Research Paper Award,
 *      arXiv 2503.10385.
 *
 *   2. **Project itself** — generated per-state with the current
 *      permalink URL embedded, so the cite is round-trip-reproducible.
 *      Whoever reads the paper later can paste the URL into their
 *      browser and land on the exact same playground state the author
 *      cited from. This is the "paper-permanence" payoff promised by
 *      PLAY-04 (09-CONTEXT.md decisions section).
 *
 * Note on the Min Oo & Hartig entry: RESEARCH.md flagged this as LOW
 * confidence; the orchestrator block provides the canonical text. Do
 * not edit this constant without coordinating with the planner — the
 * exact title, booktitle, year and arXiv ID are the user-facing
 * deliverable referenced from the landing hero AND the modal.
 */

/**
 * BibTeX entry for Min Oo & Hartig (ESWC 2025).
 *
 * Per the orchestrator `downstream_consumer` block + 09-CONTEXT.md.
 * Best Research Paper Award at ESWC 2025. arXiv: 2503.10385.
 */
export const HARTIG_BIBTEX = `@inproceedings{minoo-hartig-2025-algebraic,
  title     = {An Algebraic Foundation for Knowledge Graph Construction},
  author    = {Min Oo, Sitt and Hartig, Olaf},
  booktitle = {Proceedings of the 22nd European Semantic Web Conference (ESWC 2025)},
  year      = {2025},
  note      = {Best Research Paper Award},
  url       = {https://arxiv.org/abs/2503.10385},
}`;

/** Plaintext citation for Min Oo & Hartig (ESWC 2025) — for non-LaTeX contexts. */
export const HARTIG_PLAINTEXT =
  'Min Oo, Sitt and Hartig, Olaf. An Algebraic Foundation for Knowledge Graph Construction. ' +
  'In Proceedings of the 22nd European Semantic Web Conference (ESWC 2025). ' +
  'Best Research Paper Award. https://arxiv.org/abs/2503.10385';

/**
 * Deployment target for the playground. The cite snippets embed
 * `{PLAYGROUND_URL}#{permalink}` so anyone with the BibTeX can land on
 * the exact same editor state. Per ROADMAP Phase 10's deployment target.
 */
const PLAYGROUND_URL = 'https://playground.kanzo.dev';

/**
 * Sanitise a permalink fragment into a BibTeX-safe cite-key suffix.
 *
 * BibTeX cite keys must not contain `{`, `}`, `,`, whitespace, or `=`.
 * base64url permalinks already avoid `+`, `/`, and `=`, but may contain
 * `-` and `_` which BibTeX accepts. We keep the first 8 alphanumeric
 * characters — short enough to paste-friendly, long enough to be
 * effectively unique for a given mapping (~62^8 ≈ 2 × 10^14 keys).
 */
function shortCiteKey(permalink: string): string {
  const trimmed = permalink.slice(0, 16).replace(/[^a-zA-Z0-9]/g, '');
  // Fall back to a constant suffix when the permalink is unusably short.
  return trimmed.length > 0 ? trimmed.slice(0, 8) : 'snapshot';
}

/**
 * Build a BibTeX entry for the current playground state.
 *
 * @param permalink - the base64url-encoded permalink fragment (without
 *                    the leading `#`); pass an empty string and the cite
 *                    key falls back to `snapshot` (still a valid BibTeX
 *                    key, but the URL section won't round-trip state).
 * @param year      - publication year (defaults to current).
 */
export function buildPlaygroundBibtex(
  permalink: string,
  year: number = new Date().getFullYear(),
): string {
  const shortKey = shortCiteKey(permalink);
  const url = `${PLAYGROUND_URL}/#${permalink}`;
  return `@misc{fossil-playground-${shortKey},
  title  = {Fossil Playground — Mapping Snapshot},
  author = {Fossil Contributors},
  year   = {${year}},
  url    = {${url}},
  note   = {Generated from the Fossil playground; permalink encodes the editor state.},
}`;
}

/** Plaintext citation for the current playground snapshot. */
export function buildPlaygroundPlaintext(
  permalink: string,
  year: number = new Date().getFullYear(),
): string {
  const url = `${PLAYGROUND_URL}/#${permalink}`;
  return `Fossil Contributors. Fossil Playground — Mapping Snapshot. ${year}. ${url}`;
}
