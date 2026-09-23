/**
 * Diagnostics — the half of a fossil editor that nothing else provides.
 *
 * `@kanzo-tech/ui`'s `CodeEditor` composes rather than needing a fork: it takes
 * `extensions` in a live-reconfigured `Compartment`, and every `@codemirror/*` is
 * an optional peer. What it does not have is any diagnostics support at all —
 * `invalid?: boolean` draws a red border and `@codemirror/lint` is not even a
 * peer. Its theme block styles `.cm-diagnostic-error` and `.cm-lintPoint-warning`
 * and leaves a comment saying the lint panel «is a caller's dependency rather than
 * ours». This is that caller.
 *
 * ## The squiggle is not the only rendering
 *
 * `CheckRow.related` carries the other places one diagnostic points at — two
 * mappings minting one identity, the line of the `.shex` that declares a violated
 * constraint, which is in ANOTHER FILE. CodeMirror's `Diagnostic` has no
 * cross-file concept, so those are folded into the message as `→ uri:line`
 * suffixes rather than dropped. A reader following one gets a location; a reader
 * ignoring it loses nothing.
 */
import { linter, type Diagnostic } from '@codemirror/lint';
import type { EditorState, Extension } from '@codemirror/state';

import { rangeOf, type Position } from './positions.js';

/** LSP `DiagnosticSeverity`, as `fossil-wasm` emits it on `CheckRow.severity`. */
const SEVERITY: Readonly<Record<number, Diagnostic['severity']>> = {
  1: 'error',
  2: 'warning',
  3: 'info',
  4: 'hint',
};

/** The `CheckRow` shape, restated structurally so this module imports no runtime.
 *  `@fossil-lang/wasm` is the definition; anything with these fields works. */
export interface CheckRowLike {
  uri: string;
  range: { start: Position; end: Position };
  severity: number;
  message: string;
  related?: { uri: string; range: { start: Position; end: Position }; message: string }[];
}

// The clamping this module used to do itself is `positions.ts`'s now, because
// hover and goto-def need exactly the same arithmetic and exactly the same
// clamp: a position is computed against the text as it was when the request went
// out, and the user may have deleted that line before the answer lands.

/**
 * Project `CheckRow`s onto CodeMirror `Diagnostic`s against a given document.
 *
 * `uri` filters to one buffer. The workspace-wide `check()` returns rows for every
 * open file — the `.shex` shape document the playground had to open, any second
 * program — and painting another file's errors onto this one's text is worse than
 * showing nothing.
 */
export function toDiagnostics(
  state: EditorState,
  rows: readonly CheckRowLike[],
  uri: string,
): Diagnostic[] {
  const out: Diagnostic[] = [];
  for (const row of rows) {
    if (row.uri !== uri) continue;
    const { from, to } = rangeOf(state, row.range);
    const related = (row.related ?? [])
      .map((r) => `\n  → ${r.uri}:${r.range.start.line + 1}: ${r.message}`)
      .join('');
    out.push({
      from,
      to,
      severity: SEVERITY[row.severity] ?? 'error',
      source: 'fossil',
      message: row.message + related,
    });
  }
  return out;
}

/** What {@link fossilLinter} calls to get rows. Synchronous or not — the wasm
 *  surface is synchronous, but a host driving an LSP Worker over `postMessage`
 *  is not, and both should be able to use this. */
export type CheckSource = (
  text: string,
) => readonly CheckRowLike[] | Promise<readonly CheckRowLike[]>;

/** Options for {@link fossilLinter}. */
export interface LinterOptions {
  /** The URI the buffer was opened under. Rows for other URIs are dropped. */
  uri: string;
  /**
   * Milliseconds of quiet before a check runs. Default 120.
   *
   * **This is the coalescing, and it is the library's job rather than the app's.**
   * `@codemirror/lint` waits out the delay and then waits for the promise before
   * scheduling again, so no two checks overlap no matter how fast anyone types —
   * which is what `apps/playground` was doing by hand with a `setTimeout` and a
   * `busy` flag to stay clear of a wasm re-entrancy defect. That defect is fixed
   * in `crates/fossil-wasm` now, but the scheduling was always the right shape for
   * an editor: an LSP client coalesces `didChange` rather than emitting one
   * notification per keystroke. Putting it here means a host cannot forget it.
   *
   * 120 ms is below where an editor stops feeling live and well above a fast
   * typist's inter-key interval. `@codemirror/lint`'s own default is 750.
   */
  delay?: number;
  /** Called with every batch, before filtering. A host that renders its own
   *  diagnostics panel — the playground does — reads it here rather than running
   *  a second check. */
  onDiagnostics?: (rows: readonly CheckRowLike[]) => void;
}

/**
 * The linter extension: squiggles, the hover tooltip, and whatever lint UI the
 * host installed alongside.
 *
 * Requires `@codemirror/lint` at runtime — the one peer that is not optional here,
 * because there is no diagnostics story without it.
 */
export function fossilLinter(source: CheckSource, options: LinterOptions): Extension {
  return linter(
    async (view) => {
      const text = view.state.doc.toString();
      let rows: readonly CheckRowLike[];
      try {
        rows = await source(text);
      } catch (cause) {
        // A refused check is a diagnostic in its own right, and a silent one is
        // how "the editor stopped underlining things" becomes a mystery. The
        // wasm surface's own busy error says what to do; show it.
        return [
          {
            from: 0,
            to: Math.min(1, view.state.doc.length),
            severity: 'error' as const,
            source: 'fossil',
            message: `fossil check failed: ${String(cause)}`,
          },
        ];
      }
      options.onDiagnostics?.(rows);
      return toDiagnostics(view.state, rows, options.uri);
    },
    { delay: options.delay ?? 120 },
  );
}
