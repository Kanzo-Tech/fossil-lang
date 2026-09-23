/**
 * Syntax highlighting for Fossil, driven by the compiler's own lexer.
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
 * `hello.fossil` the playground opens that is microseconds. `maxLength` caps it —
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
import type { TokenRow } from '@fossil-lang/types';

import { byteToUtf16Mapper } from './offsets.js';
import { tagFor } from './tags.js';

/** The two wasm entry points this plugin needs. Injected rather than imported so
 *  the package does not decide when the module is initialised — see `index.ts`. */
export interface TokenSource {
  /** `@fossil-lang/wasm`'s `tokenize`. */
  tokenize: (text: string) => TokenRow[];
  /** `@fossil-lang/wasm`'s `tokenKinds` — the legend that makes `kind` readable. */
  tokenKinds: () => readonly string[];
}

/** Options for {@link fossilHighlighting}. */
export interface HighlightOptions {
  /** Documents longer than this (in UTF-16 code units) are not tokenized.
   *  Default 200_000 — comfortably past any hand-written mapping. */
  maxLength?: number;
}

const DEFAULT_MAX_LENGTH = 200_000;

/**
 * The decoration pass. Exported for the tests, which assert over ranges rather
 * than over a rendered DOM.
 */
export function buildDecorations(
  view: EditorView,
  source: TokenSource,
  maxLength: number,
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const text = view.state.doc.toString();
  if (text.length === 0 || text.length > maxLength) return builder.finish();

  let rows: TokenRow[];
  let legend: readonly string[];
  try {
    rows = source.tokenize(text);
    legend = source.tokenKinds();
  } catch {
    // The module is not initialised yet, or the host tore it down. A highlighter
    // that throws takes the editor's whole update cycle with it; one that returns
    // no decorations leaves plain text and repaints on the next change.
    return builder.finish();
  }

  const toUnits = byteToUtf16Mapper(text);
  for (const row of rows) {
    const tag = tagFor(legend, row.kind);
    if (tag === null) continue;
    const cls = highlightingFor(view.state, [tag]);
    if (!cls) continue;
    const from = toUnits(row.start);
    const to = toUnits(row.end);
    // `RangeSetBuilder` requires strictly sorted, non-empty ranges. The lexer
    // emits both in order, but a zero-width token would still be a runtime throw.
    if (to <= from) continue;
    builder.add(from, to, Decoration.mark({ class: cls }));
  }
  return builder.finish();
}

/**
 * The highlighting extension.
 *
 * Recomputes on a document change and on a viewport change — the latter because
 * `highlightingFor` reads a facet, and a host that swaps its `HighlightStyle` (a
 * light/dark toggle, say) dispatches a reconfigure rather than a doc change.
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
