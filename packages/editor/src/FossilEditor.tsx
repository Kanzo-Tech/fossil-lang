/**
 * FossilEditor — hand-rolled React wrapper around `EditorView`.
 *
 * Per RESEARCH.md Pattern 4 (~50-LOC wrapper instead of `@uiw/react-codemirror`):
 * the React ecosystem's CodeMirror wrappers add little value beyond what a
 * tightly-controlled ref + two `useEffect` calls deliver, and they impose
 * their own version constraints on `@codemirror/*` that frequently conflict
 * with peerDep ranges (RESEARCH.md Pitfall 10).
 *
 * Lifecycle:
 *   1. First effect (mount): construct `EditorState` from initial value +
 *      extensions, instantiate `EditorView`, attach updateListener that
 *      forwards doc changes to `onChange`. Cleanup destroys the view.
 *   2. Second effect (value control): if the controlled `value` diverges
 *      from the editor's current doc (e.g. host reset the example), dispatch
 *      a single replace transaction. Skipped when they match — prevents
 *      cursor jitter on every keystroke.
 *
 * The `extensions` dep on the first effect means changing the extensions
 * array tears the editor down + rebuilds it. Consumers (`FossilPlayground`)
 * memoise the extensions array via `useMemo` to keep the editor stable
 * across renders.
 *
 * Phase 11 (11-02) extensions: the prop surface now accepts an OPTIONAL
 * `extensions[]` plus `lspTransport: Transport | null` + `resolver?:
 * ConnectionResolver`. When `extensions` is provided the caller's pre-
 * composed array wins (the playground path — preserves zero behavioural
 * change for v0.1.x consumers). When `extensions` is OMITTED the component
 * auto-composes `[fossil({ resolver }), buildLspExtension(transport)?]`
 * from the standalone-usability props (per ADR-0036 Transport-superset).
 */

import { useEffect, useMemo, useRef } from 'react';
import { EditorState, type Extension } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import type { LSPClient } from '@codemirror/lsp-client';
import { fossil } from '@fossil-lang/codemirror-fossil';
import type { ConnectionResolver } from '@fossil-lang/types';
import type { InferredDescriptor } from '@fossil-lang/introspect';
import type { Transport } from './transports/types.js';
import { buildLspExtension } from './lsp/buildLspExtension.js';

export interface FossilEditorProps {
  /** Controlled doc content. The editor reflects this when it diverges from
   *  the live doc; otherwise local edits flow uninterrupted. */
  value: string;
  /** Forwarded on every doc change (debounced by CodeMirror's own update
   *  batching — typically once per keystroke). */
  onChange?: (value: string) => void;
  /** Optional pre-composed extensions. When provided the caller controls
   *  the full composition (playground path — pre-composes
   *  `fossil({ resolver }) + languageServerSupport(client, uri, 'fossil')`).
   *  When omitted the component auto-composes from `lspTransport` + `resolver`. */
  extensions?: Extension[];
  /** Pluggable LSP transport (per ADR-0036). `null` ⇒ read-only static
   *  (no LSP wiring; syntactic highlighting + @-autocomplete still work). */
  lspTransport: Transport | null;
  /** Connection resolver for `@`-prefix autocomplete (CONN-01..03 carryover). */
  resolver?: ConnectionResolver;
  /** Inferred source descriptors to register with the LSP server (drives
   *  source-field completion). Produced by `@fossil-lang/introspect`; on change
   *  the editor sends one `fossil/registerInferredDescriptor` notification per
   *  descriptor. Auto-compose path only (when `extensions` is omitted) — the
   *  `extensions` path's host owns its own client + registration. */
  descriptors?: InferredDescriptor[];
  /** Optional class for theming hooks. Defaults to `'fossil-editor'`. */
  className?: string;
}

export function FossilEditor({
  value,
  onChange,
  extensions,
  lspTransport,
  resolver,
  descriptors,
  className,
}: FossilEditorProps): JSX.Element {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  // The LSP client the auto-compose path creates — held so the descriptor
  // effect can push `fossil/registerInferredDescriptor` notifications. `null`
  // on the `extensions` path (that host owns its own client).
  const lspClientRef = useRef<LSPClient | null>(null);
  // Stash the latest onChange in a ref so the mount-effect doesn't re-run when
  // the host passes a fresh closure on every render.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  // Composition rule:
  //   - extensions provided  → playground path: caller owns the composition
  //   - extensions omitted   → standalone path: auto-compose fossil({resolver})
  //                            + buildLspExtension(transport) when transport != null
  const composed = useMemo<Extension[]>(() => {
    if (extensions) {
      lspClientRef.current = null;
      return extensions;
    }
    const exts: Extension[] = fossil({ resolver });
    if (lspTransport) {
      const { extension, client } = buildLspExtension(lspTransport);
      lspClientRef.current = client;
      exts.push(extension);
    } else {
      lspClientRef.current = null;
    }
    return exts;
  }, [extensions, lspTransport, resolver]);

  // Mount/teardown effect — keyed on the composed extensions array identity.
  // The host-supplied path memoises; the auto-composed path stabilises via
  // the useMemo above (lspTransport + resolver are reference-stable in
  // typical consumers).
  useEffect(() => {
    if (!hostRef.current) return;
    const state = EditorState.create({
      doc: value,
      extensions: [
        ...composed,
        EditorView.updateListener.of((u) => {
          if (u.docChanged) onChangeRef.current?.(u.state.doc.toString());
        }),
      ],
    });
    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // Deliberate: exclude `value` so typing doesn't tear down the editor.
    // Controlled updates flow through the second effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [composed]);

  // Controlled value sync — only fires when the host changes value
  // externally (e.g., Reset, Load Example). Skipped when the host echoes
  // back the value we already emitted via onChange.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
    }
  }, [value]);

  // Register inferred source descriptors with the LSP server (auto-compose path).
  // Fires on descriptor change AND on `composed` change (the latter recreates
  // the client, so descriptors must be re-pushed to the new one). Best-effort +
  // idempotent: the worker queues pre-boot messages, and register overwrites.
  useEffect(() => {
    const client = lspClientRef.current;
    if (!client || !descriptors) return;
    for (const descriptor of descriptors) {
      client.notification('fossil/registerInferredDescriptor', descriptor);
    }
  }, [descriptors, composed]);

  return <div ref={hostRef} className={className ?? 'fossil-editor'} />;
}
