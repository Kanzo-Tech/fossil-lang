/**
 * Hover — the type of what you wrote, and the type the shape demands of it.
 *
 * `fossil_ide::hover_bidirectional` is the answer and it is *bidirectional*,
 * which is the whole point of hovering in a mapping language: the first block is
 * the type of the expression under the cursor and where that type came from, and
 * the second is what the target shape requires of the predicate it is being
 * written to. Two ends of a program, in one tooltip, because
 * [both ends are types](/docs/design/types).
 *
 * ## Why the source takes the text
 *
 * The same reason {@link CheckSource} does. The wasm workspace answers about the
 * text of the last `updateFile`, and hover fires on mouse-move while the checker
 * is debounced — so a source that only took `(line, character)` would let a host
 * ask about text it had not pushed, and get a range one keystroke wrong. Passing
 * the text makes the push and the query one step. The host still decides whether
 * that push costs anything: `apps/playground/src/check.ts` compares against what
 * it last sent and skips the call, so the common case is a string comparison.
 *
 * ## The Markdown
 *
 * `hover_bidirectional`'s output uses exactly three constructs — a fenced
 * ```` ```fossil ```` block, an `*italic*` line, and `` `inline code` `` — and
 * {@link renderMarkdown} handles those three. It is not a Markdown renderer and
 * does not want to be one; a host that needs one passes `render`. Anything else
 * the compiler ever emits falls through as literal text, which is legible and
 * wrong-looking rather than silently dropped.
 */
import { EditorView, hoverTooltip, type Tooltip } from '@codemirror/view';
import type { Extension } from '@codemirror/state';

import { positionOf, rangeOf, type Range } from './positions.js';

/** The `HoverRow` shape, restated structurally so this module imports no
 *  runtime. `@fossil-lang/wasm` is the definition. */
export interface HoverRowLike {
  markdown: string;
  range: Range;
}

/** What {@link fossilHover} calls. Synchronous or not — the wasm surface is
 *  synchronous, a host driving a Worker is not, and both belong here. */
export type HoverSource = (
  text: string,
  line: number,
  character: number,
) => HoverRowLike | null | Promise<HoverRowLike | null>;

/** Options for {@link fossilHover}. */
export interface HoverOptions {
  /** Milliseconds the pointer must rest before a hover is requested. Default
   *  300 — CodeMirror's own `hoverTime` default, and the number every editor
   *  has converged on. */
  hoverTime?: number;
  /** Render the Markdown into a DOM node. Default {@link renderMarkdown}. */
  render?: (markdown: string) => HTMLElement;
}

/**
 * Render the three constructs `hover_bidirectional` emits.
 *
 * Fenced blocks become `<pre class="cm-fossil-hover-code">`, `*text*` becomes
 * `<em>`, `` `text` `` becomes `<code>`, and everything else is text. No
 * `innerHTML` anywhere: every node is constructed, so a type name containing
 * `<` is a type name and not markup.
 */
export function renderMarkdown(markdown: string): HTMLElement {
  const root = document.createElement('div');
  root.className = 'cm-fossil-hover';

  // Fences alternate: text, code, text, code… A trailing unterminated fence is
  // treated as code, which is what it looks like on the screen anyway.
  const parts = markdown.split(/^```[^\n]*\n?/m);
  parts.forEach((part, i) => {
    if (part === '') return;
    if (i % 2 === 1) {
      const pre = document.createElement('pre');
      pre.className = 'cm-fossil-hover-code';
      pre.textContent = part.replace(/\n$/, '');
      root.appendChild(pre);
      return;
    }
    for (const line of part.split('\n')) {
      if (line.trim() === '') continue;
      const p = document.createElement('p');
      p.className = 'cm-fossil-hover-text';
      appendInline(p, line);
      root.appendChild(p);
    }
  });
  return root;
}

/** `*em*` and `` `code` ``, in one pass, with everything else as text. */
function appendInline(into: HTMLElement, line: string): void {
  const pattern = /\*([^*]+)\*|`([^`]+)`/g;
  let last = 0;
  for (let m = pattern.exec(line); m !== null; m = pattern.exec(line)) {
    if (m.index > last) into.appendChild(document.createTextNode(line.slice(last, m.index)));
    const em = m[1];
    const code = m[2];
    const node = document.createElement(em !== undefined ? 'em' : 'code');
    node.textContent = em ?? code ?? '';
    into.appendChild(node);
    last = m.index + m[0].length;
  }
  if (last < line.length) into.appendChild(document.createTextNode(line.slice(last)));
}

/**
 * Layout for the nodes {@link renderMarkdown} builds, and **layout only**.
 *
 * The package still ships no theme: not one colour, font or border is set here.
 * What is set is the spacing of markup this file invented, because a `<pre>`
 * inside a tooltip arrives with the browser's default 1em margins and reads as
 * two paragraphs with a gap. `EditorView.baseTheme` is the lowest precedence
 * CodeMirror has, so a host that wants any of it different simply says so —
 * `@kanzo-tech/ui` already styles `.cm-tooltip` itself, and this sits under
 * that.
 */
const hoverBaseTheme = EditorView.baseTheme({
  '.cm-fossil-hover': { padding: '4px 8px', maxWidth: '32em' },
  '.cm-fossil-hover-code': { margin: '2px 0', whiteSpace: 'pre-wrap' },
  '.cm-fossil-hover-text': { margin: '2px 0' },
});

/**
 * The hover extension.
 *
 * Requires `@codemirror/view`, which every CodeMirror host has by construction.
 * The tooltip is anchored to the range the compiler reported rather than to the
 * pointer, so hovering anywhere in `User.name` underlines all of `User.name`.
 */
export function fossilHover(source: HoverSource, options: HoverOptions = {}): Extension {
  const render = options.render ?? renderMarkdown;
  return [hoverBaseTheme, hoverTooltip(
    async (view, pos): Promise<Tooltip | null> => {
      const { line, character } = positionOf(view.state, pos);
      let row: HoverRowLike | null;
      try {
        row = await source(view.state.doc.toString(), line, character);
      } catch {
        // A refused hover is not worth a tooltip saying so — unlike a refused
        // check, which is the linter's whole output and shows up as a
        // diagnostic. Here the honest rendering of "no answer" is no tooltip.
        return null;
      }
      if (!row) return null;
      const { from, to } = rangeOf(view.state, row.range);
      return {
        pos: from,
        end: to,
        above: true,
        create: () => ({ dom: render(row.markdown) }),
      };
    },
    { hoverTime: options.hoverTime ?? 300 },
  )];
}
