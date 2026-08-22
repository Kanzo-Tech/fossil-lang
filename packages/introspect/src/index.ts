/**
 * @fossil-lang/introspect — source-binding schema introspection (the one home).
 *
 * "Given a `.fossil` mapping + a way to read its sources, produce an
 * `InferredDescriptor` per source binding" is a single fossil capability. It
 * was copied across the playground hook and ad-hoc host code; this package is
 * the canonical TS home, and the host injects only the DATA PLANE (URL
 * resolution + a DuckDB executor), the same shape as `@fossil-lang/graph`'s
 * injected `DuckExecutor`.
 *
 * Framework-agnostic + zero @fossil-lang deps (a true leaf). The React glue
 * and the descriptor→LSP-worker push are the HOST's, not ours — fossil ships
 * no UI; the host decides how `resolve`/`query` reach its cloud + DuckDB.
 *
 * The primitive union below is the wire form of `fossil-graph-schema`'s
 * `Primitive`. A value outside the union is rejected when the descriptor is
 * registered.
 *
 * `fossil-engine` does the same job natively, and the two must agree on three
 * things. Two of them stopped being an agreement and became one source: the
 * constructors that exist and the reader each picks are generated from
 * `catalogue.bnf` into `catalogue.generated.ts`, and the Rust reads the same
 * file through `fossil_base::providers`. The third — the DuckDB→primitive
 * table — is still written twice, and `tests/rust-parity.test.ts` reads that
 * crate's source and goes red when the two diverge.
 *
 * Zero @fossil-lang deps still holds: the generated module is a file in this
 * package, not a dependency on another one. `packages/executor` gets its own
 * projection of the same rows for the same reason.
 */

import { NATIVE_READERS, NATIVE_ROWS, type NativeRow } from "./catalogue.generated.js";

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
   * The source URI exactly as the program writes it (`data/users.csv` from
   * `users := io.csv("data/users.csv")`) — the key the compiler looks the
   * descriptor up under, and NOT the resolved URL this package fetched. The
   * written URI is the only string the host and the checker both see: the
   * checker has neither the `@conn` credentials nor the program directory the
   * resolution needs.
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
 * row added there reaches this type, the reader table below and the scrape
 * alternation at once, and none of the three can be the one that was forgotten.
 */
export type SourceFormat = NativeRow;

/** A source binding scraped from a `.fossil` mapping. */
export interface SourceRef {
  sourceName: string;
  /** Which `io.` constructor wrote it — it chooses the DuckDB reader. */
  format: SourceFormat;
  url: string;
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
  if (upper === "TIMESTAMP" || upper === "DATETIME") return "date_time";
  if (upper === "TIME") return "time";
  // VARCHAR / TEXT / STRING + any unrecognised type fall back to string.
  return "string";
}

/**
 * The source-binding pattern. Exported because it is the thing the parity
 * guard compares against `fossil-engine`'s, and because a caller that wants to
 * ask "does this text bind any source?" should not write a second one.
 *
 * Not a shared `RegExp` instance: `g` carries `lastIndex`, so one object
 * reused across calls skips matches.
 */
export const SOURCE_REF_PATTERN =
  `(\\w[\\w\\d_]*)\\s*:=\\s*io\\.(${NATIVE_ROWS.join("|")})\\(\\s*['"]([^'"]+)['"]`;

/**
 * Scrape source-binding RHS URLs from a `.fossil` text.
 *
 * LIMITATIONS (regex placeholder; an AST walk supersedes it): no multi-line
 * constructor, no interleaved comments between `:=` and `io.csv(`, no
 * backslash-escaped quotes inside the URL string.
 */
export function extractSourceRefs(text: string): SourceRef[] {
  const re = new RegExp(SOURCE_REF_PATTERN, "g");
  const out: SourceRef[] = [];
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m[1] && m[2] && m[3]) {
      out.push({ sourceName: m[1], format: m[2] as SourceFormat, url: m[3] });
    }
  }
  return out;
}

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
 * The canonical DESCRIBE SQL for a resolved source URL. The URL is
 * single-quote-escaped (a SQL string literal, not a prepared parameter), and
 * `format` is the constructor the binding was written with — there is no
 * default, because a defaulted reader is how a `.parquet` source ends up read
 * as CSV.
 */
export function describeSql(url: string, format: SourceFormat): string {
  const escaped = url.replace(/'/g, "''");
  return `DESCRIBE SELECT * FROM ${READERS[format]}('${escaped}')`;
}

/**
 * Build the descriptor a `DESCRIBE` produced for one source. Keyed by the URI
 * the program wrote, not the binding name and not the URL `resolve` returned.
 * Columns with empty/missing names are dropped (defensive against malformed
 * rows).
 *
 * `freshnessToken` is what the host knows about the source's state — an ETag
 * or `Last-Modified` off the fetch that fed the DESCRIBE is the cheap one in a
 * browser. Omitted, it is `""`: never fresh, so the compiler re-introspects
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

/**
 * Host-injected data plane. `resolve` turns a scraped ref into a readable URL
 * string (signed cloud URL, bundled example, server proxy — the host's call);
 * `query` runs a SQL string and returns the rows (a DuckDB-WASM connection, a
 * server round-trip — the host's call). This is the `@fossil-lang/graph`
 * injection pattern applied to introspection.
 */
export interface IntrospectIO {
  resolve(ref: SourceRef): Promise<string> | string;
  query(sql: string): Promise<readonly DescribeRow[]> | readonly DescribeRow[];
  /**
   * Optional freshness token for the source behind `resolvedUrl` — an ETag, a
   * `Last-Modified`, a version id. Only the host can produce one cheaply,
   * because only the host knows how it fetched the file. Absent, descriptors
   * carry `""` and the compiler re-introspects on every compile.
   */
  freshness?(ref: SourceRef, resolvedUrl: string): Promise<string> | string;
  /** Optional per-source failure sink; defaults to `console.warn`. */
  onWarn?(message: string, err: unknown): void;
}

function defaultWarn(message: string, err: unknown): void {
  // eslint-disable-next-line no-console
  console.warn(message, err);
}

/**
 * Introspect every source binding in `mappingText` and return the descriptors.
 * Best-effort: a per-source failure (unreachable URL, DuckDB error) is logged
 * and skipped, never thrown — the editor degrades gracefully to no field
 * completion for that source. The host registers the returned descriptors with
 * the editor / LSP worker.
 */
export async function introspect(
  mappingText: string,
  io: IntrospectIO,
): Promise<InferredDescriptor[]> {
  const warn = io.onWarn ?? defaultWarn;
  const out: InferredDescriptor[] = [];
  for (const ref of extractSourceRefs(mappingText)) {
    try {
      const url = await io.resolve(ref);
      const rows = await io.query(describeSql(url, ref.format));
      out.push(buildDescriptor(ref.url, rows, await io.freshness?.(ref, url)));
    } catch (err) {
      warn(
        `[introspect] source \`${ref.sourceName}\` (url=\`${ref.url}\`) failed`,
        err,
      );
    }
  }
  return out;
}
