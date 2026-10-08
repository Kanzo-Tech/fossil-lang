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
 * `Diagnostic.related` carries the other places one diagnostic points at — two
 * mappings minting one identity, the line of the `.shex` that declares a violated
 * constraint, which is in ANOTHER FILE. CodeMirror's `Diagnostic` has no
 * cross-file concept, so those are folded into the message as `→ uri:line`
 * suffixes rather than dropped. A reader following one gets a location; a reader
 * ignoring it loses nothing. `help` goes under the message as a `help:` line, the
 * way `fossil check` prints it.
 *
 * ## The two repairs are actions
 *
 * A row's `didYouMean` and `suggestion` are `fossil-ide`'s two quick fixes as data,
 * so each becomes a `Diagnostic.actions` entry that applies the edit: replace the
 * misspelt name, or replace the diagnostic's range with the source `suggestion`
 * carries.
 */
import { linter, type Action, type Diagnostic as LintDiagnostic } from '@codemirror/lint';
import type { EditorState, Extension } from '@codemirror/state';
import { FossilError, helpUrl, isFossilError, type Diagnostic, type Problem } from '@fossil-lang/types';
import { diagnosticOf } from '@fossil-lang/types/internal';
import type { FossilProgram } from '@fossil-lang/wasm';

import { rangeOf } from './positions.js';

/** LSP `DiagnosticSeverity`, as `fossil-wasm` emits it on `Diagnostic.severity`. */
const SEVERITY: Readonly<Record<Diagnostic['severity'], LintDiagnostic['severity']>> = {
  1: 'error',
  2: 'warning',
  3: 'info',
  4: 'hint',
};


/**
 * A problem as a diagnostic's text: its title, its code and the page that explains it, its detail,
 * its help. The code is what a person types into a search, so it is on the first line.
 */
export function problemMessage(problem: Problem): string {
  const help = problem.help === undefined ? '' : `\nhelp: ${problem.help}`;
  return `${problem.title} [${problem.code}]: ${problem.detail}${help}\nsee ${helpUrl(problem.code)}`;
}

/** Replace `range` — measured against the text the row was computed from — with `insert`. */
function replace(name: string, range: Diagnostic['range'], insert: string): Action {
  return {
    name,
    apply(view) {
      const { from, to } = rangeOf(view.state, range);
      view.dispatch({ changes: { from, to, insert } });
    },
  };
}

function actionsOf(row: Diagnostic): Action[] {
  const actions: Action[] = [];
  if (row.didYouMean !== undefined) {
    const { range, replacement } = row.didYouMean;
    actions.push(replace(`Replace with \`${replacement}\``, range, replacement));
  }
  const { suggestion } = row;
  if (suggestion !== undefined) {
    actions.push({
      name: 'Split mapping into one per ShEx OneOf disjunct',
      apply: (view, from, to) => view.dispatch({ changes: { from, to, insert: suggestion } }),
    });
  }
  return actions;
}

// The clamping this module used to do itself is `positions.ts`'s now, because
// hover and goto-def need exactly the same arithmetic and exactly the same
// clamp: a position is computed against the text as it was when the request went
// out, and the user may have deleted that line before the answer lands.

/**
 * Project fossil's `Diagnostic`s onto CodeMirror's against a given document.
 *
 * `uri` filters to one buffer: painting another file's errors onto this one's text is worse than
 * showing nothing.
 */
export function toDiagnostics(
  state: EditorState,
  rows: readonly Diagnostic[],
  uri: string,
): LintDiagnostic[] {
  const out: LintDiagnostic[] = [];
  for (const row of rows) {
    if (row.uri !== uri) continue;
    const { from, to } = rangeOf(state, row.range);
    const help = row.help === undefined ? '' : `\nhelp: ${row.help}`;
    const related = (row.related ?? [])
      .map((r) => `\n  → ${r.uri}:${r.range.start.line + 1}: ${r.message}`)
      .join('');
    const actions = actionsOf(row);
    out.push({
      from,
      to,
      severity: SEVERITY[row.severity] ?? 'error',
      source: 'fossil',
      message: row.message + help + related,
      ...(actions.length === 0 ? {} : { actions }),
    });
  }
  return out;
}

/** What a check threw, as fossil's failure: itself if fossil raised it, `internal/bug` if not. */
function failureOf(cause: unknown): FossilError {
  return isFossilError(cause)
    ? cause
    : FossilError.of('internal/bug', { what: 'the check failed outside fossil' }, { cause });
}

/**
 * A check that never answered, as the one diagnostic a host counts: `cause` by its code on the
 * first character of `uri`, and `internal/bug` when fossil did not raise it.
 *
 * {@link fossilLinter} draws a failed check with exactly this diagnostic and hands it to
 * `onDiagnostics`, so a host's panel and the squiggle say the same thing and a program nobody could
 * check never reads as clean. A host whose failure happens before there is a linter —
 * `openProgram` rejecting — reports it with the same call rather than a copy of it.
 */
export function uncheckedDiagnostic(uri: string, cause: unknown): Diagnostic {
  return diagnosticOf(uri, failureOf(cause).problem);
}

/** Options for {@link fossilLinter}. */
export interface LinterOptions {
  /**
   * Milliseconds of quiet before a check runs. Default 120.
   *
   * **This is the coalescing, and it is the library's job rather than the app's.**
   * `@codemirror/lint` waits out the delay and then waits for the promise before
   * scheduling again, so no two checks overlap no matter how fast anyone types —
   * which is what a host used to do by hand with a `setTimeout` and a
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
   *  diagnostics panel reads it here rather than running
   *  a second check. A check that threw is a batch too — its one
   *  {@link uncheckedDiagnostic} — so the host never wraps `diagnostics` to learn of it. */
  onDiagnostics?: (rows: readonly Diagnostic[]) => void;
}

/**
 * The linter extension: squiggles, the hover tooltip, and whatever lint UI the
 * host installed alongside.
 *
 * Requires `@codemirror/lint` at runtime — the one peer that is not optional here,
 * because there is no diagnostics story without it.
 */
export function fossilLinter(
  program: Pick<FossilProgram, 'uri' | 'diagnostics'>,
  options: LinterOptions = {},
): Extension {
  return linter(
    async (view) => {
      let rows: readonly Diagnostic[];
      try {
        rows = await program.diagnostics();
      } catch (cause) {
        // A refused check is a diagnostic in its own right, and a silent one is
        // how "the editor stopped underlining things" becomes a mystery. It goes
        // through `onDiagnostics` like any batch, and is drawn by its code — the
        // row's `message` is the problem's detail, and a squiggle with no code
        // on it is one nobody can search for.
        const failure = failureOf(cause);
        const row = uncheckedDiagnostic(program.uri, failure);
        options.onDiagnostics?.([row]);
        const { from, to } = rangeOf(view.state, row.range);
        return [
          {
            from,
            to,
            severity: 'error' as const,
            source: 'fossil',
            message: problemMessage(failure.problem),
          },
        ];
      }
      options.onDiagnostics?.(rows);
      return toDiagnostics(view.state, rows, program.uri);
    },
    { delay: options.delay ?? 120 },
  );
}
