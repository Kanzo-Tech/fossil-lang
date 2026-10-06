/**
 * @fossil-lang/introspect — source-binding schema introspection (the one home).
 *
 * "Given the sources a program reads, produce an `InferredDescriptor` per
 * source" is a single fossil capability. Which sources a program reads is not
 * this package's question: fossil answers it from the AST
 * (`FossilWorkspace.sources`), with every `@conn/path` already expanded into
 * a locator and the connection it goes through. This package reads each one
 * through the host's DuckDB under the credential the host vends for that
 * connection (`@fossil-lang/storage`), and owns the DESCRIBE SQL, the
 * DuckDB→primitive table and the descriptor shape. The engine is the host's,
 * which is why DuckDB-WASM is not a peer.
 *
 * The descriptor and its `Primitive` are `@fossil-lang/types`', generated from
 * the Rust that reads them.
 *
 * `fossil-introspect` does the same job natively. The constructors that exist,
 * the reader each picks, its option keyword and the DuckDB→primitive table are
 * generated from `catalogue.bnf` into `catalogue.generated.ts`, and into the
 * Rust from the same file.
 */

import { mount, type Mount } from "@fossil-lang/storage";
import {
  FossilError,
  attachCause,
  isFossilError,
  type Engine,
  type Host,
  type InferredColumn,
  type InferredDescriptor,
  type Primitive,
  type Problem,
  type ProgramSource,
} from "@fossil-lang/types";

import {
  NATIVE_OPTIONS,
  NATIVE_READERS,
  NATIVE_ROWS,
  duckdbPrimitive,
  type NativeRow,
} from "./catalogue.generated.js";

/**
 * The `io/` source constructors an introspecting host can DESCRIBE — the rows
 * `catalogue.bnf` gives a `reads native <fn>`.
 *
 * It was a hand-written union of three literals. It is `catalogue.bnf`'s now,
 * through `cargo xtask catalogue`, which is the same source the Rust reads: a
 * row added there reaches this type and the reader table below at once.
 */
export type SourceFormat = NativeRow;

type NativeSource = ProgramSource & { format: SourceFormat };

/** Whether this package can DESCRIBE a source — a materialised row
 *  (`io.rdf`) takes its schema from its shape instead. */
function isNative(source: ProgramSource): source is NativeSource {
  return (NATIVE_ROWS as readonly string[]).includes(source.format);
}

/** A single row from DuckDB's `DESCRIBE SELECT * FROM <reader>(...)`. */
export interface DescribeRow {
  column_name?: unknown;
  column_type?: unknown;
}

/**
 * The canonical DESCRIBE SQL for a readable path. The path is
 * single-quote-escaped (a SQL string literal, not a prepared parameter), and
 * `format` is the constructor the binding was written with, and it chooses the
 * reader: there is no default, because a defaulted reader is how a `.parquet`
 * source ends up read as CSV, and a JSON array read as CSV is one column called
 * `[`.
 *
 * `option` is the reader option the binding named, and the DESCRIBE has to
 * carry it or it describes a different file than the run reads: a
 * pipe-delimited CSV read with a comma is ONE column called `id|name|city`.
 */
export function describeSql(
  url: string,
  format: SourceFormat,
  option?: string,
): string {
  const escaped = url.replace(/'/g, "''");
  const keyword = NATIVE_OPTIONS[format];
  const args =
    option !== undefined && keyword !== undefined
      ? `, ${keyword}='${option.replace(/'/g, "''")}'`
      : "";
  return `DESCRIBE SELECT * FROM ${NATIVE_READERS[format]}('${escaped}'${args})`;
}

/**
 * Build the descriptor a `DESCRIBE` produced for one source. Keyed by what
 * the program wrote, not the binding name and not the URL that was read.
 * Columns with empty/missing names are dropped (defensive against malformed
 * rows).
 *
 * `freshnessToken` is what the host knows about the source's state — an ETag
 * or a `Last-Modified` is the cheap one. Omitted, it is `""`: never fresh, so the compiler re-introspects
 * every time. That is the correct default for a host that has not wired one,
 * and it is not a hash of the columns — a token derived from the answer cannot
 * tell you whether to ask the question.
 */
export function buildDescriptor(
  uri: string,
  describeRows: readonly DescribeRow[],
  freshnessToken = "",
): InferredDescriptor {
  const columns: InferredColumn[] = describeRows
    .map((r) => ({
      name: String(r.column_name ?? ""),
      primitive: duckdbPrimitive(String(r.column_type ?? "")),
    }))
    .filter((c) => c.name.length > 0);
  return { uri, columns, freshness_token: freshnessToken };
}

/** What a host gives introspection: its credentials and the page's engine. */
export interface IntrospectIO {
  host: Host;
  engine: Engine;
  /**
   * A token for the state of the source — an ETag, a `Last-Modified`, a version id. Only the host
   * can produce one cheaply. Absent, descriptors carry `""` and the compiler re-introspects on
   * every compile.
   */
  freshness?(source: ProgramSource): Promise<string> | string;
  /** Stops it: the credential requests and every DESCRIBE, rejecting with the signal's reason. */
  signal?: AbortSignal;
}

/** A source introspection could not describe, and why. */
export interface UndescribedSource {
  source: ProgramSource;
  problem: Problem;
}

/**
 * Describe every native source and return the descriptors, in `sources` order. The host is asked
 * once per connection named, and the credential it vends is given back when this returns.
 *
 * Best-effort: a source the host vends nothing for, or one DuckDB cannot read, is skipped, never
 * thrown, and answered in `undescribed` with its problem — the editor degrades to no field
 * completion for that source. A source with no connection is read as it is only when it is a public
 * `http(s)` URL. The host hands the whole result to `FossilProgram.registerIntrospection`: the
 * descriptors type the program, and each undescribed source is a warning of `check` at its call.
 *
 * @throws {FossilError} only what giving the credentials back raised; an abort rejects with the
 *   signal's reason.
 */
export async function introspect(
  sources: readonly ProgramSource[],
  io: IntrospectIO,
): Promise<{ descriptors: InferredDescriptor[]; undescribed: UndescribedSource[] }> {
  const { signal } = io;
  const native = sources.filter(isNative);
  const undescribed: UndescribedSource[] = [];
  if (native.length === 0) return { descriptors: [], undescribed };

  const mounts = new Map<string, Promise<Mount>>();
  const mountOf = (connection: string): Promise<Mount> => {
    let pending = mounts.get(connection);
    if (pending === undefined) {
      pending = mount(io.engine, io.host, { connection }, "read", { signal });
      mounts.set(connection, pending);
    }
    return pending;
  };
  let outcome: InferredDescriptor[] | undefined;
  let failure: unknown;
  try {
    const described = await Promise.all(
      native.map(async (source) => {
        try {
          let name = source.locator;
          if (source.connection !== undefined) {
            const mounted = await mountOf(source.connection);
            name = (await mounted.files([source.locator]))[0] ?? name;
          } else if (!/^https?:\/\//.test(source.locator)) {
            throw FossilError.of(
              "storage/no-route",
              { locator: source.locator },
            );
          }
          const described = await io.engine.query(describeSql(name, source.format, source.option), {
            signal: signal ?? new AbortController().signal,
          });
          const names = described.getChild("column_name");
          const types = described.getChild("column_type");
          const rows: DescribeRow[] = Array.from({ length: described.numRows }, (_, i) => ({
            column_name: names?.get(i),
            column_type: types?.get(i),
          }));
          return buildDescriptor(source.key, rows, await io.freshness?.(source));
        } catch (err) {
          signal?.throwIfAborted();
          const problem = isFossilError(err)
            ? err.problem
            : FossilError.of("engine/failed", {}, {
                cause: err,
              }).problem;
          undescribed.push({ source, problem });
          return undefined;
        }
      }),
    );
    outcome = described.filter((d): d is InferredDescriptor => d !== undefined);
    undescribed.sort((a, b) => sources.indexOf(a.source) - sources.indexOf(b.source));
  } catch (cause) {
    failure = cause;
  }
  const closed = await Promise.allSettled(
    // eslint-disable-next-line no-restricted-syntax -- a mount that failed is already a source's problem above, so there is nothing of it to close
    [...mounts.values()].map(async (pending) => (await pending.catch(() => undefined))?.close()),
  );
  const cleanup = closed.filter((c) => c.status === "rejected").map((c) => c.reason as unknown);
  if (failure !== undefined) {
    for (const c of cleanup) attachCause(failure, c);
    throw failure;
  }
  if (cleanup.length > 0) throw cleanup[0];
  return { descriptors: outcome!, undescribed };
}
