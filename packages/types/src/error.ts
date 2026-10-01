/**
 * **One error, from Rust to the screen** — `/docs/design/errors`.
 *
 * Everything that leaves fossil for JavaScript, thrown or returned, is a {@link Problem}: a code
 * from one catalogue, the typed `data` that code carries, a fixed `title`, a `detail` for a person
 * that nothing parses, and a chain of causes. What is thrown is a {@link FossilError} carrying it;
 * the wasm crates build theirs in Rust (`fossil_graph_schema::js::to_js`), the TypeScript packages
 * build theirs here, and the two are the same object.
 *
 * A host branches on `code` — `isFossilError(e, 'run/over-budget')` — and reads `data`. It never
 * reads `message` for anything but showing it.
 */

import { CODES, TITLES, type Code, type ProblemData } from './problem.gen.js';

export { CODES, TITLES, type Code, type ProblemData } from './problem.gen.js';

/** How bad a problem is. */
export type Severity = 'error' | 'warning' | 'info';

/** An error fossil did not raise — DataFusion's, DuckDB's, a host's — kept as a cause. */
export interface Foreign {
  /** The error's own name. */
  name: string;
  /** The error's own text. */
  detail: string;
}

/**
 * A compile diagnostic a failure carries — what `run/does-not-compile` relates: a problem, and
 * where in the program. `span` is file-absolute bytes.
 */
export type Related<C extends Code = Code> = {
  [K in C]: {
    code: K;
    data: ProblemData[K];
    title: string;
    detail: string;
    severity: Severity;
    help?: string;
    span?: { start: number; end: number };
  };
}[C];

/** The fields every problem has, whatever its code. */
export interface ProblemBase {
  /** The identifier (RFC 9457 `type`). */
  code: Code;
  /** Fixed per code (RFC 9457 `title`). */
  title: string;
  /** For a person; never parsed (RFC 9457 `detail`). */
  detail: string;
  severity: Severity;
  help?: string;
  related?: Related[];
  /** A nested problem, or an error fossil did not raise. */
  cause?: Problem | Foreign;
}

/** A problem, discriminated by `code`: narrowing on the code narrows `data` to that code's fields. */
export type Problem<C extends Code = Code> = {
  [K in C]: ProblemBase & { code: K; data: ProblemData[K] };
}[C];

const codes: ReadonlySet<string> = new Set(CODES);

/** Plain data with a catalogued code — a problem on the wire, not an error carrying one. */
function isProblem(value: unknown): value is Problem {
  return (
    typeof value === 'object' &&
    value !== null &&
    !(value instanceof Error) &&
    typeof (value as { code?: unknown }).code === 'string' &&
    codes.has((value as { code: string }).code)
  );
}

function causeOf(cause: Problem | Foreign | undefined): Error | undefined {
  if (cause === undefined) return undefined;
  if (isProblem(cause)) return FossilError.from(cause);
  const error = new Error(cause.detail);
  error.name = cause.name;
  return error;
}

/** What {@link FossilError.of} takes beside the code, its data and its detail. */
export interface Occurrence {
  help?: string;
  related?: Related[];
  /**
   * What caused it. A {@link Problem} is kept as one; any other value — an engine's `Error`, a
   * host's rejection — is kept whole as the error's `cause`, and reaches `problem.cause` as
   * `{ name, detail }`.
   */
  cause?: unknown;
}

/**
 * The error fossil throws. `name` is `'FossilError'`, `message` is the problem's `detail`, and
 * `code`, `data` and `problem` are the rest; `cause` is a nested `FossilError` for a cause fossil
 * raised and the original error for one it did not.
 *
 * Test for one with {@link isFossilError}, never `instanceof`: an error built in wasm and one built
 * here, or two copies of this package in one bundle, are different classes.
 */
export class FossilError<C extends Code = Code> extends Error {
  override readonly name = 'FossilError' as const;
  readonly code: C;
  readonly data: ProblemData[C];
  readonly title: string;
  readonly help: string | undefined;
  readonly related: Related[] | undefined;
  /** The whole problem, as plain data — what crosses a `postMessage` and what {@link from} takes. */
  readonly problem: Problem<C>;

  constructor(problem: Problem<C>, options?: { cause?: unknown }) {
    const cause = options && 'cause' in options ? options.cause : causeOf(problem.cause);
    super(problem.detail, cause === undefined ? undefined : { cause });
    this.code = problem.code;
    this.data = problem.data as ProblemData[C];
    this.title = problem.title;
    this.help = problem.help;
    this.related = problem.related;
    this.problem = problem;
  }

  /** Rebuild the error from its plain data — the far side of a worker, a stored run's problem. */
  static from<C extends Code>(problem: Problem<C>): FossilError<C> {
    return new FossilError(problem);
  }

  /**
   * Raise a code from TypeScript: the problem is built with the code's {@link TITLES | title}, and a
   * cause that is not a problem stays the error's own `cause`, whole.
   */
  static of<C extends Code>(
    code: C,
    data: ProblemData[C],
    detail: string,
    occurrence: Occurrence = {},
  ): FossilError<C> {
    const { help, related, cause } = occurrence;
    const wire = cause === undefined ? undefined : toWire(cause);
    const problem = {
      code,
      data,
      title: TITLES[code],
      detail,
      severity: 'error',
      ...(help === undefined ? {} : { help }),
      ...(related === undefined ? {} : { related }),
      ...(wire === undefined ? {} : { cause: wire }),
    } as Problem<C>;
    if (cause === undefined) return new FossilError(problem);
    return new FossilError(problem, { cause: isProblem(cause) ? FossilError.from(cause) : cause });
  }
}

/** Any cause as it goes on the wire: a problem as itself, any other thrown value `{ name, detail }`. */
function toWire(cause: unknown): Problem | Foreign {
  if (isFossilError(cause)) return cause.problem;
  if (isProblem(cause)) return cause;
  if (cause instanceof Error) return { name: cause.name, detail: cause.message };
  return { name: 'Error', detail: String(cause) };
}

/**
 * Whether `e` is a {@link FossilError} — of `code`, when one is given, which narrows `e.data` to
 * that code's fields. **Structural**: `name === 'FossilError'` and a `code` in the catalogue, so an
 * error built by the wasm crates passes as one built here does.
 */
export function isFossilError(e: unknown): e is FossilError;
export function isFossilError<C extends Code>(e: unknown, code: C): e is FossilError<C>;
export function isFossilError(e: unknown, code?: Code): boolean {
  if (typeof e !== 'object' || e === null) return false;
  const { name, code: has } = e as { name?: unknown; code?: unknown };
  return name === 'FossilError' && typeof has === 'string' && codes.has(has) && (code === undefined || has === code);
}

/** The published documentation. `fossil_graph_schema::Problem::help_url` builds the same link, and
 *  a test there reads these two constants. */
const DOCS = 'https://kanzo-tech.github.io/fossil-lang';
/** The error index within it: a page per code, the code as its route. */
const INDEX = 'docs/errors';

/**
 * The page that explains `code`. `base` is where the documentation is served — the published site
 * unless a deployment serves its own copy. There is no URL on the wire, because the prefix is the
 * deployment's to choose.
 */
export function helpUrl(code: Code, base: string = DOCS): string {
  return `${base.replace(/\/+$/, '')}/${INDEX}/${code}`;
}
