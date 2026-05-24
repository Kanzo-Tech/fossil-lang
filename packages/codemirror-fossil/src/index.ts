/**
 * @fossil-lang/codemirror-fossil — CodeMirror 6 language extension for Fossil.
 *
 * Per ADR-0028 + ADR-0030, this package is publishable standalone (no
 * `@fossil-lang/playground` dependency, no React, no LSP machinery). The
 * Keasy migration target — its existing CodeMirror editor migrates off the
 * hand-rolled `StreamLanguage` lexer onto this package's WASM-delegated
 * syntactic highlighting via `@fossil-lang/wasm` `tokenize()`.
 *
 * The composition root is {@link fossil}; sub-extensions are exported
 * individually so consumers can mix-and-match (e.g. include syntactic
 * highlighting but disable `@`-autocomplete).
 *
 * LSP integration (diagnostics, hover, semantic-tokens overlay, goto-def,
 * completion-from-LSP) is deliberately out of scope for this package —
 * 08-09's `@fossil-lang/playground` composes this package with
 * `@codemirror/lsp-client` (ADR-0032) to layer LSP features on top.
 */

import type { Extension } from '@codemirror/state';
import type { ConnectionResolver } from '@fossil-lang/types';

import { fossilAutocomplete } from './autocomplete.js';
import { fossilLanguageSupport } from './parser.js';

/** Options for the {@link fossil} composition root. */
export interface FossilExtensionOpts {
  /**
   * Optional {@link ConnectionResolver} — drives `@`-prefixed autocomplete
   * via `resolver.list()`. Omit to disable connector autocomplete entirely
   * (highlighting still works).
   */
  resolver?: ConnectionResolver;
}

/**
 * Default Fossil CodeMirror extension. Composes:
 *   - syntactic highlighting via `@fossil-lang/wasm` `tokenize` (ADR-0030)
 *   - `@`-prefixed connector autocomplete via the supplied
 *     {@link ConnectionResolver} (ADR-0029)
 *
 * Drop this into any `EditorState.create({ extensions: [...] })` call and the
 * editor becomes Fossil-aware. No LSP machinery — for LSP-driven features,
 * use `@fossil-lang/playground` which composes this package with
 * `@codemirror/lsp-client`.
 */
export function fossil(opts: FossilExtensionOpts = {}): Extension[] {
  return [fossilLanguageSupport(), fossilAutocomplete(opts.resolver)];
}

// Sub-extension exports — consumers can compose à la carte.
export {
  fossilLanguage,
  fossilLanguageSupport,
  fossilStreamParser,
} from './parser.js';
export type { FossilState } from './parser.js';

export {
  fossilAutocomplete,
  fossilAutocompleteSource,
} from './autocomplete.js';

export { FossilKind, KIND_TO_TAG, kindToTagName } from './tags.js';
