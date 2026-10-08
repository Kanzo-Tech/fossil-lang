/**
 * Syntax highlighting for Fossil, driven by the compiler's own lexer and, when
 * the host wires it, the compiler's semantic tokens laid over it.
 *
 * ## Two layers, the rust-analyzer-over-TextMate arrangement
 *
 * The lexical pass (`tokenize()`) needs no workspace and is always there: it is
 * the baseline. The semantic pass names what the lexer cannot — a shape
 * from a binding from a column, a declaration from a use, the `@connection` of a
 * reference inside its string — and wins where it speaks. Where it repeats what
 * the lexer already said (a keyword, a string) the lexer's finer tag stays; see
 * `LEXICAL_KINDS`. If the semantic call throws, the lexical layer is painted
 * alone and nothing else changes.
 *
 * ## Why a `ViewPlugin` and not a `StreamLanguage`
 *
 * The version of this package deleted in `873cbc0` wrapped `tokenize()` in a
 * `StreamParser`, and the seam showed. CodeMirror drives a `StreamParser` line by
 * line and hands it a `StringStream`; `tokenize()` wants the whole document and
 * returns document-absolute offsets. Bridging the two meant caching a token array
 * on the parser state and re-tokenizing whenever `stream.string.length` disagreed
 * with a remembered length — its own comment called that «a coarse but reliable
 * heuristic», and it is coarse: two edits that cancel out in length leave the
 * cache in place and the colours stale.
 *
 * A `ViewPlugin` matches the shape of what we actually have. One call per document
 * change, over the text CodeMirror already holds, producing one `DecorationSet`.
 * No cache invalidation to get wrong, because there is no cache.
 *
 * ## Why `highlightingFor` and not a stylesheet of our own
 *
 * `highlightingFor(state, [tag])` asks the ACTIVE `HighlightStyle` — whichever the
 * host installed — for the class it uses for that tag. So this plugin has no
 * colours in it at all, and an editor that already has a theme keeps it. That is
 * what lets `@kanzo-tech/ui`'s `CodeEditor` render fossil in its own palette
 * without either package knowing about the other: it installs `kanzoHighlighting`,
 * we ask that style what a `tags.keyword` looks like there.
 *
 * A host with no highlight style installed gets no classes and plain text, which
 * is the honest result rather than a fallback palette fighting the theme.
 *
 * ## Cost, and the ceiling
 *
 * O(document) per change: one wasm call plus one `RangeSetBuilder` pass. On the
 * `hello.fossil` that is microseconds. `maxLength` caps it —
 * past that the plugin emits nothing rather than tokenizing a megabyte on every
 * keystroke, and a document that large is not what this editor is for.
 */
import { highlightingFor } from '@codemirror/language';
import { RangeSetBuilder, type Extension } from '@codemirror/state';
import {
  Decoration,
  ViewPlugin,
  type DecorationSet,
  type EditorView,
  type ViewUpdate,
} from '@codemirror/view';
import type { SemanticToken, Token } from '@fossil-lang/types';
import type { FossilProgram } from '@fossil-lang/wasm';

import { offsetOf } from './positions.js';
import { LEXICAL_KINDS, TAG_BY_NAME, semanticTagFor } from './tags.js';

/** What the plugin asks: the lexer over the text, and the semantic tokens of the text last
 *  updated. Synchronous, because a decoration set is. */
export type TokenSource = Pick<FossilProgram, 'tokenize' | 'semanticTokens'>;

/** Options for {@link fossilHighlighting}. */
export interface HighlightOptions {
  /** Documents longer than this (in UTF-16 code units) are not tokenized.
   *  Default 200_000 — comfortably past any hand-written mapping. */
  maxLength?: number;
}

const DEFAULT_MAX_LENGTH = 200_000;

/** One decoration to place: a range and the class the active style gives it. */
interface Mark {
  from: number;
  to: number;
  cls: string;
}

/** The decoration pass: the ranges to paint, before any DOM. */
export function buildDecorations(
  view: EditorView,
  source: TokenSource,
  maxLength: number,
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const text = view.state.doc.toString();
  if (text.length === 0 || text.length > maxLength) return builder.finish();

  const lexical = lexicalMarks(view, source, text);
  for (const mark of overlay(lexical, semanticMarks(view, source))) {
    builder.add(mark.from, mark.to, Decoration.mark({ class: mark.cls }));
  }
  return builder.finish();
}

function lexicalMarks(view: EditorView, source: TokenSource, text: string): Mark[] {
  let rows: Token[];
  try {
    rows = source.tokenize(text);
  } catch {
    // The module is not initialised yet, or the host tore it down. A highlighter
    // that throws takes the editor's whole update cycle with it; one that returns
    // no decorations leaves plain text and repaints on the next change.
    return [];
  }

  const marks: Mark[] = [];
  for (const { kind, start: from, end: to } of rows) {
    const tag = TAG_BY_NAME[kind];
    if (tag === undefined) continue;
    const cls = highlightingFor(view.state, [tag]);
    if (!cls) continue;
    // `RangeSetBuilder` requires strictly sorted, non-empty ranges. The lexer
    // emits both in order, but a zero-width token would still be a runtime throw.
    if (to <= from) continue;
    marks.push({ from, to, cls });
  }
  return marks;
}

/** A semantic mark, and whether it may only fill a gap the lexer left. */
interface SemanticMark extends Mark {
  fillsOnly: boolean;
}

function semanticMarks(view: EditorView, source: TokenSource): SemanticMark[] {
  let rows: readonly SemanticToken[];
  try {
    rows = source.semanticTokens();
  } catch {
    // Busy or not booted: the lexical layer is the answer until the next change.
    return [];
  }

  const marks: SemanticMark[] = [];
  let last = 0;
  for (const row of rows) {
    const tag = semanticTagFor(row.kind, row.modifiers);
    if (tag === null) continue;
    const cls = highlightingFor(view.state, [tag]);
    if (!cls) continue;
    const from = offsetOf(view.state, row.range.start);
    const to = offsetOf(view.state, row.range.end);
    if (to <= from || from < last) continue;
    marks.push({ from, to, cls, fillsOnly: LEXICAL_KINDS.has(row.kind) });
    last = to;
  }
  return marks;
}

/**
 * `top` painted over `base`: every lexical mark loses the stretches a semantic
 * mark covers — so a string keeps its colour on either side of the connection
 * carved out of it — except that a mark which only fills is dropped wherever the
 * lexer already painted. Both inputs are sorted and non-overlapping, and so is
 * the result.
 */
function overlay(base: Mark[], top: SemanticMark[]): Mark[] {
  let b = 0;
  const kept = top.filter((t) => {
    if (!t.fillsOnly) return true;
    while (b < base.length && base[b]!.to <= t.from) b++;
    return b === base.length || base[b]!.from >= t.to;
  });

  const out: Mark[] = [...kept];
  let k = 0;
  for (const mark of base) {
    while (k < kept.length && kept[k]!.to <= mark.from) k++;
    let cursor = mark.from;
    for (let i = k; i < kept.length && kept[i]!.from < mark.to; i++) {
      if (kept[i]!.from > cursor) out.push({ from: cursor, to: kept[i]!.from, cls: mark.cls });
      cursor = Math.max(cursor, kept[i]!.to);
    }
    if (cursor < mark.to) out.push({ from: cursor, to: mark.to, cls: mark.cls });
  }
  return out.sort((x, y) => x.from - y.from);
}

/**
 * The highlighting extension.
 *
 * Recomputes on a document change and on a viewport change — the latter because
 * `highlightingFor` reads a facet, and a host that swaps its `HighlightStyle` (a
 * light/dark toggle, say) dispatches a reconfigure rather than a doc change. A
 * viewport change re-asks `semanticTokens` of the same revision.
 */
export function fossilHighlighting(
  source: TokenSource,
  options: HighlightOptions = {},
): Extension {
  const maxLength = options.maxLength ?? DEFAULT_MAX_LENGTH;
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = buildDecorations(view, source, maxLength);
      }

      update(update: ViewUpdate): void {
        if (update.docChanged || update.viewportChanged) {
          this.decorations = buildDecorations(update.view, source, maxLength);
        }
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}
