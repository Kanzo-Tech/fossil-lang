/**
 * What a program references, and what running it would need — answered before anything runs.
 *
 * ## The claim this module exists to make visible
 *
 * A host cannot ask for credentials it does not know it needs. `io.csv("@warehouse/users.csv")`
 * names a connection; the host has to know that alias BEFORE the run, because the run is when
 * the credential is used, and discovering a missing connection by executing into a failure is
 * the alternative this replaces.
 *
 * `refs()` answers it by PARSING. Not by executing, not by connecting, not by reading the file
 * it names. `providers()` answers "what constructors exist, and which extensions does each
 * read" out of the registry, with no db at all. Both are the same code the native
 * `fossil refs` / `fossil providers` run — `fossil_lineage::source_refs` and
 * `fossil_lineage::providers`, one crate, two hosts — so the browser's answer and the CLI's
 * cannot diverge.
 *
 * ## Why this file may call the surface on every keystroke, and `check.ts` may not
 *
 * `check.ts` drives `FossilPlayground`, a persistent workspace: `updateFile` mutates a Salsa
 * input behind a `RefCell`, and re-entering it throws *"recursive use of an object detected"*
 * and poisons the workspace for the session. Hence its 120 ms debounce and its `busy` guard.
 *
 * `refs` is a **free function** and shares none of that. `fossil-wasm`'s `refs_native` builds a
 * throwaway single-file db per call and drops it — it never touches the editor's workspace, and
 * says so in its own docblock. There is no object to re-enter, so there is nothing to guard and
 * nothing to debounce. That is not a detail: it is the same parse-only property the UI is
 * claiming, showing up as an API affordance. This module therefore deliberately does NOT
 * debounce, and must not grow a call to anything on `FossilPlayground`.
 *
 * ## What "costs no engine" means in bytes
 *
 * `refs` and `providers` live in `fossil_wasm_bg.wasm` — the checker bundle, already loaded to
 * type-check as you type. Answering costs nothing beyond it. What it does NOT touch is the
 * engine: DuckDB-WASM and `fossil-df-wasm`. {@link engineDeferred} reports that gap from the
 * app's own measurements rather than from this comment.
 */
import { providers, refs, type ProviderInfo, type SourceRefInfo } from '@fossil-lang/wasm';

export type { ProviderInfo, SourceRefInfo };

/** A connection a program names, and every reference that targets it. */
export interface ConnectionNeed {
  /** The `@conn` alias, or `null` for direct paths (which need no credential). */
  alias: string | null;
  /** The references reaching through it, in program order. */
  refs: SourceRefInfo[];
}

/** One parse-only answer about a program. */
export interface Lineage {
  /** Every external reference, deduplicated by `fossil_lineage::source_refs`. */
  refs: SourceRefInfo[];
  /**
   * The distinct connection set — the credential prompt, derived. `alias: null` groups the
   * direct paths, which are the ones needing no credential at all. This is the DISTINCT
   * `connection` of the typed lineage, never a regex over the script text.
   */
  needs: ConnectionNeed[];
  /** Wall-clock milliseconds for the `refs()` call itself. Sub-millisecond is the point. */
  elapsedMs: number;
  /** Set when the parse itself refused — a program mid-keystroke is often not yet a program. */
  error: string | null;
}

const EMPTY: Lineage = { refs: [], needs: [], elapsedMs: 0, error: null };

/**
 * How often the surface has actually been called, and the worst it has cost.
 *
 * Counted HERE rather than in the component, and the reason is not tidiness. `main.tsx` renders
 * under `StrictMode`, which double-invokes render bodies and `useMemo` callbacks to surface
 * exactly this kind of impurity — a counter incremented during render would read 2× the
 * keystrokes, in a panel whose entire argument is that its numbers are measurements. Counting
 * at the call site makes the number true by construction: it is the number of times `refs`
 * ran, whoever asked and however many times React chose to ask.
 */
let calls = 0;
let worstMs = 0;

export function surfaceStats(): { calls: number; worstMs: number } {
  return { calls, worstMs };
}

/**
 * Parse `program` and report its lineage. Synchronous, allocation-cheap, safe to call on
 * every keystroke — see the re-entrancy note above for why this one may and `check` may not.
 *
 * Never throws: a half-typed program is the normal case in an editor, and a lineage panel that
 * blanks out mid-word is worse than one that holds its last good answer. The caller decides
 * what to do with {@link Lineage.error}.
 */
export function lineageOf(program: string): Lineage {
  const started = performance.now();
  let rows: SourceRefInfo[];
  calls += 1;
  try {
    rows = refs(program);
  } catch (cause) {
    return { ...EMPTY, elapsedMs: performance.now() - started, error: String(cause) };
  }
  const elapsedMs = performance.now() - started;
  worstMs = Math.max(worstMs, elapsedMs);

  // Group by alias, preserving first-seen order. `null` (direct paths) sorts last: it is the
  // "needs nothing" bucket and the aliases are the answer worth reading first.
  const byAlias = new Map<string | null, SourceRefInfo[]>();
  for (const row of rows) {
    const bucket = byAlias.get(row.connection);
    if (bucket) bucket.push(row);
    else byAlias.set(row.connection, [row]);
  }
  const needs = [...byAlias.entries()]
    .map(([alias, refs]) => ({ alias, refs }))
    .sort((a, b) => (a.alias === null ? 1 : b.alias === null ? -1 : 0));

  return { refs: rows, needs, elapsedMs, error: null };
}

/**
 * The provider table — what a connector picker is populated from.
 *
 * Cached because it is a pure projection of a static registry: the answer cannot change within
 * a session, and re-crossing the wasm boundary per render to be told `csv` reads `.csv` again
 * would undercut the claim that this surface is cheap.
 */
let cachedProviders: ProviderInfo[] | null = null;

export function providerTable(): ProviderInfo[] {
  if (!cachedProviders) cachedProviders = providers();
  return cachedProviders;
}

/** Which providers could read a given path, by extension. The picker's filter, and the
 *  reason `providers()` reports extensions at all. */
export function providersFor(path: string, table: readonly ProviderInfo[]): ProviderInfo[] {
  const dot = path.lastIndexOf('.');
  if (dot < 0) return [];
  const ext = path.slice(dot + 1).toLowerCase();
  return table.filter((p) => p.extensions.includes(ext));
}

/** Bytes of a bundle the app has actually measured, or `null` if it was never loaded. */
export interface Deferred {
  /** What the answer above cost: the checker bundle, already resident to type-check. */
  spentBytes: number | null;
  /** What it did NOT touch — the engine bundles, measured only once they load. */
  deferredBytes: number | null;
  /** The engine bundles by name, for the reader who wants the breakdown. */
  parts: { name: string; bytes: number | null }[];
}

/**
 * The byte gap between "parse the program" and "run the program", from the app's own
 * measurements rather than from prose.
 *
 * The honest version of "costs no engine": `refs` is not free — it needs the checker wasm,
 * which is already there because the editor type-checks. It is free of the ENGINE, and the
 * engine is an order of magnitude larger. A bundle reads `null` until it has loaded, which is
 * itself the evidence: before the first Run, the deferred column is unmeasured because nothing
 * fetched it.
 */
export function engineDeferred(
  checkerBytes: number | null,
  duckdbBytes: number | null,
  executorBytes: number | null,
): Deferred {
  const parts = [
    { name: 'duckdb-wasm', bytes: duckdbBytes },
    { name: 'fossil-df-wasm', bytes: executorBytes },
  ];
  const known = parts.map((p) => p.bytes).filter((b): b is number => b !== null);
  return {
    spentBytes: checkerBytes,
    deferredBytes: known.length ? known.reduce((a, b) => a + b, 0) : null,
    parts,
  };
}
