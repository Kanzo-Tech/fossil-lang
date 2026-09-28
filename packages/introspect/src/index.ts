/**
 * @fossil-lang/introspect — source-binding schema introspection (the one home).
 *
 * "Given the sources a program reads, produce an `InferredDescriptor` per
 * source" is a single fossil capability. Which sources a program reads is not
 * this package's question: fossil answers it from the AST
 * (`FossilPlayground.sources`), with every `@conn/path` already expanded into
 * a locator and the connection it goes through. This package reads each one
 * through the host's DuckDB under the credential the host vends for that
 * connection (`@fossil-lang/storage`), and owns the DESCRIBE SQL, the
 * DuckDB→primitive table and the descriptor shape. The engine is the host's,
 * which is why DuckDB-WASM is not a peer.
 *
 * The primitive union below is the wire form of `fossil-graph-schema`'s
 * `Primitive`. A value outside the union is rejected when the descriptor is
 * registered.
 *
 * `fossil-introspect` does the same job natively. The constructors that exist
 * and the reader each picks are generated from `catalogue.bnf` into
 * `catalogue.generated.ts`, and the Rust reads the same file through
 * `fossil_base::providers`. The DuckDB option keyword and the DuckDB→primitive
 * table are still written twice, and so is the DESCRIBE itself;
 * `tests/rust-parity.test.ts` reads that crate's source and goes red when the
 * two diverge.
 */

import { mount, type Mount } from "@fossil-lang/storage";
import type { Engine, Host, ProgramSource } from "@fossil-lang/types";

import {
  NATIVE_READERS,
  NATIVE_ROWS,
  type NativeRow,
} from "./catalogue.generated.js";

/**
 * The Fossil primitive lattice (mirror `@fossil-lang/wasm`'s
 * `InferredPrimitive` — structurally identical so `introspect()` output flows
 * straight into `FossilPlayground.registerInferredDescriptor`). Collapsing the
 * two into one source is still open.
 */
export type InferredPrimitive =
  | "string"
  | "integer"
  | "float"
  | "bool"
  | "date"
  | "date_time"
  | "time"
  | "g_year"
  | "any_uri";

export interface InferredColumn {
  name: string;
  primitive: InferredPrimitive;
}

export interface InferredDescriptor {
  /**
   * The source's `ProgramSource.key` — what the program wrote
   * (`@warehouse/users.csv`), not its locator. It is
   * the key the compiler looks the descriptor up under, and a connection that
   * moves changes the locator without moving the key.
   */
  uri: string;
  /** Ordered, position-significant columns. */
  columns: InferredColumn[];
  /**
   * Opaque token identifying the state of the source. The compiler's cache
   * compares it and re-introspects when it moves; nothing interprets it.
   * `""` means "this host cannot cheaply tell", which is never fresh.
   */
  freshness_token: string;
}

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

/** Map a DuckDB column-type string onto the Fossil lattice. */
export function duckdbTypeToFossilPrimitive(t: string): InferredPrimitive {
  const upper = t.trim().toUpperCase();
  if (
    upper === "INTEGER" ||
    upper === "BIGINT" ||
    upper === "INT" ||
    upper === "SMALLINT" ||
    upper === "TINYINT" ||
    upper === "HUGEINT"
  ) {
    return "integer";
  }
  if (upper === "DOUBLE" || upper === "FLOAT" || upper === "REAL") return "float";
  if (upper.startsWith("DECIMAL")) return "float";
  if (upper === "BOOLEAN" || upper === "BOOL") return "bool";
  if (upper === "DATE") return "date";
  // Prefix arms, mirroring `crates/fossil-introspect/src/lib.rs` — DuckDB has
  // seven spellings for an instant and this had the two that are bare names.
  // `TIMESTAMP WITH TIME ZONE` is what `read_csv_auto` infers for an ISO-8601
  // string carrying an offset, which is how LDBC-SNB dates every row it ships,
  // and it fell through to `string` here after the Rust stopped letting it.
  //
  // **`TIMESTAMP` before `TIME`, and that is the whole reason these two lines
  // are in this order**: a prefix test for `TIME` swallows every `TIMESTAMP`
  // spelling, which is the Rust's arm order for the same reason.
  if (upper.startsWith("TIMESTAMP") || upper === "DATETIME") return "date_time";
  if (upper.startsWith("TIME")) return "time";
  // VARCHAR / TEXT / STRING + any unrecognised type fall back to string.
  return "string";
}

/**
 * What DuckDB calls each row's reader option.
 *
 * **This is the ENGINE's vocabulary, and that is why it is written here rather
 * than generated.** `catalogue.bnf` names the position the PROGRAM writes;
 * `delim=` is a DuckDB named argument and `CsvReadOptions::delimiter` is a Rust
 * method taking a byte, so no one token could serve both readers. The native
 * sibling is `fossil_introspect::duckdb_option_keyword`, and it is the same
 * table for the same reason — each half speaks to its own DuckDB.
 */
const DUCKDB_OPTION_KEYWORD: Partial<Record<SourceFormat, string>> = {
  csv: "delim",
};

/**
 * The DuckDB table function each constructor reads through.
 *
 * The constructor chooses the reader, and it must: a JSON array read as CSV
 * introspects to one column named after its first line, so a file opening with
 * a bare `[` yields a schema whose only column is `[` and every real column
 * comes back unknown.
 *
 * The table is generated from the `native <fn>` token in `catalogue.bnf` — the
 * same token `fossil_base::NativeReader::table_function` gives back on the Rust
 * side. It used to be three literals here and three more in `fossil-engine`,
 * kept in step by a parity test that read the Rust with a regex.
 */
const READERS: Record<SourceFormat, string> = NATIVE_READERS;

/**
 * The canonical DESCRIBE SQL for a readable path. The path is
 * single-quote-escaped (a SQL string literal, not a prepared parameter), and
 * `format` is the constructor the binding was written with — there is no
 * default, because a defaulted reader is how a `.parquet` source ends up read
 * as CSV.
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
  const keyword = DUCKDB_OPTION_KEYWORD[format];
  const args =
    option !== undefined && keyword !== undefined
      ? `, ${keyword}='${option.replace(/'/g, "''")}'`
      : "";
  return `DESCRIBE SELECT * FROM ${READERS[format]}('${escaped}'${args})`;
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
      primitive: duckdbTypeToFossilPrimitive(String(r.column_type ?? "")),
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
  /** Per-source failure sink; defaults to `console.warn`. */
  onWarn?(message: string, err: unknown): void;
}

function defaultWarn(message: string, err: unknown): void {
  // eslint-disable-next-line no-console
  console.warn(message, err);
}

/**
 * Describe every native source and return the descriptors, in `sources` order. The host is asked
 * once per connection named, and the credential it vends is given back when this returns.
 *
 * Best-effort: a source the host vends nothing for, or one DuckDB cannot read, is reported
 * through `onWarn` and skipped, never thrown — the editor degrades to no field completion for that
 * source. A source with no connection is read as it is only when it is a public `http(s)` URL. The
 * host registers the returned descriptors with the checker.
 */
export async function introspect(
  sources: readonly ProgramSource[],
  io: IntrospectIO,
): Promise<InferredDescriptor[]> {
  const warn = io.onWarn ?? defaultWarn;
  const native = sources.filter(isNative);
  if (native.length === 0) return [];

  const mounts = new Map<string, Promise<Mount>>();
  const mountOf = (connection: string): Promise<Mount> => {
    let pending = mounts.get(connection);
    if (pending === undefined) {
      pending = mount(io.engine, io.host, { connection }, "read");
      mounts.set(connection, pending);
    }
    return pending;
  };
  try {
    const described = await Promise.all(
      native.map(async (source) => {
        try {
          let name = source.locator;
          if (source.connection !== undefined) {
            const mounted = await mountOf(source.connection);
            name = (await mounted.files([source.locator]))[0] ?? name;
          } else if (!/^https?:\/\//.test(source.locator)) {
            throw new Error("it names no connection and is not a public URL");
          }
          const rows = (await io.engine.query(
            describeSql(name, source.format, source.option),
          )) as DescribeRow[];
          return buildDescriptor(source.key, rows, await io.freshness?.(source));
        } catch (err) {
          warn(
            `[introspect] source \`${source.binding}\` (\`${source.key}\`) failed`,
            err,
          );
          return undefined;
        }
      }),
    );
    return described.filter((d): d is InferredDescriptor => d !== undefined);
  } finally {
    await Promise.all(
      [...mounts.values()].map(async (pending) => (await pending.catch(() => undefined))?.close()),
    );
  }
}
