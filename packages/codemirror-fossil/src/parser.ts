/**
 * CodeMirror 6 `StreamParser` for Fossil — delegates tokenization to
 * `@fossil-lang/wasm`'s `tokenize()` export (the canonical Rust lexer).
 *
 * Per ADR-0030 (Single Grammar Source of Truth), there is NO TS-side
 * lexer here — we re-tokenize the whole document on the first call and
 * walk the resulting `TokenRow[]` as CodeMirror probes per-line. The cost
 * is O(n) per change for a document of n bytes; for Fossil's typical
 * <500 LOC mappings (per RESEARCH.md Pitfall 6) that's <2ms on modern
 * hardware — acceptable for v0.1.
 *
 * Future iteration (out of scope for Phase 8): switch to a true Lezer
 * parser with incremental re-parsing. The StreamParser interface is the
 * v0.1 sweet spot — minimal code, correct semantics, no Lezer-grammar
 * authoring overhead.
 */

import { StreamLanguage, LanguageSupport } from '@codemirror/language';
import type { StreamParser } from '@codemirror/language';
import { tokenize } from '@fossil-lang/wasm';
import type { TokenRow } from '@fossil-lang/types';
import { kindToTagName } from './tags.js';

/**
 * Per-document tokenization state. `allTokens` is populated lazily on the
 * first `token()` call that sees a non-empty stream (we don't have access
 * to the full document until then); subsequent calls walk through it by
 * absolute byte offset.
 *
 * The `docSig` field guards against state staleness across edits: when the
 * document changes, CodeMirror clones the state via `copyState` for each
 * line, but a fresh top-of-document re-parse creates a new `startState`.
 * We re-tokenize whenever the stream's `string.length` doesn't match the
 * remembered total — a coarse but reliable heuristic for v0.1.
 */
export interface FossilState {
  /** All tokens for the current document, populated lazily on first use. */
  allTokens: TokenRow[];
  /** Index of the next token to emit. */
  idx: number;
  /** Absolute byte offset within the FULL document where the parser is. */
  pos: number;
  /** Length of the document the cached `allTokens` was computed against;
   *  used to detect when CodeMirror handed us a fresh document. */
  docLength: number;
}

/**
 * Eagerly re-tokenize when first called on a non-empty stream, then walk the
 * cached token array by absolute byte offset. CodeMirror invokes `token()`
 * once per token-shaped chunk per line; we advance `state.pos` to keep the
 * walker in sync with the absolute document offset (stream.pos is line-local
 * but `tokenize()` returns document-absolute byte ranges).
 *
 * Multi-line documents: the FIRST call sees the entire document via
 * `stream.string` (CodeMirror's `StringStream` exposes the full document
 * text in `string` when `StreamLanguage` is wrapping the parser). On
 * subsequent line-by-line calls, `stream.string` is the line slice — we
 * detect that by length-change and treat it as a doc swap when appropriate.
 *
 * Edge cases:
 *   - Empty input: `tokenize('')` returns `[]`; we skip to end and return null.
 *   - Gaps between tokens (whitespace dropped by the lexer): advance the
 *     stream past the gap, return null (default style).
 *   - Unknown Token kind (newly appended in a Rust release ahead of this
 *     package's update): `kindToTagName` returns null — CodeMirror falls
 *     back to the default text style. No crash.
 */
export const fossilStreamParser: StreamParser<FossilState> = {
  startState(): FossilState {
    return { allTokens: [], idx: 0, pos: 0, docLength: 0 };
  },

  copyState(state: FossilState): FossilState {
    return {
      allTokens: state.allTokens,
      idx: state.idx,
      pos: state.pos,
      docLength: state.docLength,
    };
  },

  token(stream, state): string | null {
    // (Re-)tokenize when either we haven't tokenized yet OR the stream's
    // total document length disagrees with the cached length (signals a
    // fresh document or a rebuild).
    if (state.allTokens.length === 0 || stream.string.length !== state.docLength) {
      state.allTokens = tokenize(stream.string);
      state.idx = 0;
      state.pos = 0;
      state.docLength = stream.string.length;
    }

    // Skip tokens whose ranges we've already passed.
    while (
      state.idx < state.allTokens.length &&
      state.allTokens[state.idx]!.end <= state.pos
    ) {
      state.idx++;
    }

    if (state.idx >= state.allTokens.length) {
      stream.skipToEnd();
      return null;
    }

    const tok = state.allTokens[state.idx]!;

    // Gap before this token (whitespace or anything the lexer dropped).
    // Advance the stream + position past the gap; return null for default
    // text styling.
    if (tok.start > state.pos) {
      const gap = tok.start - state.pos;
      stream.pos += gap;
      state.pos = tok.start;
      return null;
    }

    // Emit this token.
    const span = tok.end - tok.start;
    stream.pos += span;
    state.pos = tok.end;
    state.idx++;
    return kindToTagName(tok.kind);
  },

  languageData: {
    // Fossil's line-comment delimiter per grammar.bnf (and confirmed against
    // `crates/fossil-syntax/src/lexer.rs:45` — regex `//[^\n]*`).
    commentTokens: { line: '//' },
    closeBrackets: { brackets: ['(', '[', '{', '"', '`', '<'] },
  },
};

/**
 * CodeMirror `Language` for Fossil — wraps {@link fossilStreamParser} via
 * `StreamLanguage.define`. Consumers usually want {@link fossilLanguageSupport}
 * (which bundles the `Language` into a `LanguageSupport` extension) rather
 * than this raw value.
 */
export const fossilLanguage = StreamLanguage.define(fossilStreamParser);

/**
 * Canonical CodeMirror extension entry point for the Fossil language.
 * Composes the `StreamLanguage` with no language-specific support modules
 * (no indentation engine, no folding rules) for v0.1 — the StreamParser
 * carries the highlighting via `kindToTagName`.
 */
export function fossilLanguageSupport(): LanguageSupport {
  return new LanguageSupport(fossilLanguage);
}
