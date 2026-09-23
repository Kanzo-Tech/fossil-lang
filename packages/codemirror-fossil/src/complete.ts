/**
 * Completion — the receiver's members, and nothing that will not compile.
 *
 * The list is `fossil_ide::completions`', which is narrowed by the type to the
 * left of the dot rather than ranked over a catalogue: `str.` offers string
 * members and no reader, a property-key position offers the target shape's
 * predicates and no catalogue row at all, and a head that names nothing offers
 * nothing. [Tooling for humans](/docs/design/tooling-for-humans) is the
 * argument and the measurement — 51 rows before the narrowing, thirteen after.
 *
 * Nothing here re-ranks or re-filters that list. CodeMirror's own `validFor`
 * would, and it is deliberately not set: the compiler decided which rows apply
 * at this cursor, and a client-side prefix filter over the result is a second
 * opinion from the side that knows less. Every keystroke re-asks.
 *
 * ## It registers a source; it does not configure the editor
 *
 * The source goes in through `EditorState.languageData`, which is where
 * `@codemirror/autocomplete` looks when no `override` is configured. That
 * matters for the host this package was rebuilt for: `@kanzo-tech/ui`'s
 * `CodeEditor` already installs `autocompletion()` and binds
 * `completionKeymap` in its own basics, so a language layer that configured
 * autocompletion again would be reaching past the editor to set the editor's
 * options. `autocompletion()` is included too, because a bare host has to work
 * as well — calling it twice is safe by construction: every extension it
 * returns is a module-level singleton deduplicated by identity, and the config
 * facet combines rather than conflicts.
 */
import {
  autocompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from '@codemirror/autocomplete';
import { EditorState, type Extension } from '@codemirror/state';

import { positionOf } from './positions.js';

/** The `CompletionRow` shape, restated structurally so this module imports no
 *  runtime. `@fossil-lang/wasm` is the definition. */
export interface CompletionRowLike {
  label: string;
  /** The LSP `CompletionItemKind`, by name — `"function"`, `"field"`. `""`
   *  when the compiler set none. */
  kind: string;
  detail: string;
}

/** What {@link fossilCompletion} calls. Takes the text for the same reason
 *  {@link CheckSource} does — see the note in `hover.ts`. */
export type CompletionRowSource = (
  text: string,
  line: number,
  character: number,
) => readonly CompletionRowLike[] | Promise<readonly CompletionRowLike[]>;

/**
 * LSP kind name → the vocabulary CodeMirror draws an icon for.
 *
 * The two halves of this boundary each own the half they can check. Rust owns
 * *which* kinds exist and their names, and `every_lsp_kind_has_a_name` is the
 * test — that is why `kind` crosses as a name and never as a number. What
 * CodeMirror calls each one is a CodeMirror fact, so it is here: LSP's `field`
 * and `property` are both CodeMirror's `property`, and its icon set has no
 * `field`.
 *
 * A name this table does not have is `undefined`, which CodeMirror renders as
 * an option with no icon — legible, and not a crash.
 */
const CM_TYPE: Readonly<Record<string, string>> = {
  text: 'text',
  method: 'method',
  function: 'function',
  constructor: 'function',
  field: 'property',
  variable: 'variable',
  class: 'class',
  interface: 'interface',
  module: 'namespace',
  property: 'property',
  unit: 'constant',
  value: 'constant',
  enum: 'enum',
  keyword: 'keyword',
  snippet: 'text',
  color: 'constant',
  file: 'text',
  reference: 'variable',
  folder: 'text',
  enum_member: 'enum',
  constant: 'constant',
  struct: 'class',
  event: 'variable',
  operator: 'keyword',
  type_parameter: 'type',
};

/** One compiler row as a CodeMirror option. */
export function toCompletion(row: CompletionRowLike): Completion {
  const type = CM_TYPE[row.kind];
  return {
    label: row.label,
    ...(type === undefined ? {} : { type }),
    ...(row.detail === '' ? {} : { detail: row.detail }),
  };
}

/**
 * The completion source, for a host that already configures `autocompletion()`
 * and wants to place this itself.
 *
 * `from` is the start of the word being typed, so CodeMirror replaces the
 * partial token rather than inserting beside it. `explicit` requests (Ctrl-Space
 * on empty space) are answered too — the compiler returns the whole catalogue
 * there, spelled in full, which is exactly what an explicit request is for.
 */
export function fossilCompletionSource(source: CompletionRowSource) {
  return async (context: CompletionContext): Promise<CompletionResult | null> => {
    const word = context.matchBefore(/[\w.]*/);
    // Typing fires only inside a word or just after a dot; Ctrl-Space fires
    // anywhere, and the compiler answers an empty receiver with the whole
    // catalogue, which is what an explicit request is for.
    if (!context.explicit && (word === null || word.from === word.to)) return null;
    const { line, character } = positionOf(context.state, context.pos);
    let rows: readonly CompletionRowLike[];
    try {
      rows = await source(context.state.doc.toString(), line, character);
    } catch {
      // A refused completion shows as no list. The linter is where a refusal
      // becomes visible text; a popup that says "the workspace is busy" over
      // the code you are typing is worse than no popup.
      return null;
    }
    if (rows.length === 0) return null;
    // The word may be dotted (`str.tr`), and the compiler already used the head
    // to narrow: what it offers are MEMBERS, so only the segment after the last
    // dot is being replaced.
    const typed = context.state.sliceDoc(word?.from ?? context.pos, context.pos);
    const dot = typed.lastIndexOf('.');
    const from = (word?.from ?? context.pos) + (dot === -1 ? 0 : dot + 1);
    return { from, options: rows.map(toCompletion) };
  };
}

/**
 * The completion extension: the source, plus `autocompletion()` for a host that
 * has not installed it.
 */
export function fossilCompletion(source: CompletionRowSource): Extension {
  const completionSource = fossilCompletionSource(source);
  return [
    EditorState.languageData.of(() => [{ autocomplete: completionSource }]),
    autocompletion(),
  ];
}
