/**
 * @fossil-lang/introspect — source-binding schema introspection (the one home).
 *
 * "Given a `.fossil` mapping + a way to read its sources, produce an
 * `InferredDescriptor` per source binding" is a single fossil capability. It
 * previously lived duplicated across `fossil-cli` (Rust), the playground hook
 * (TS), and ad-hoc host copies. This package is the canonical TS home; the host
 * injects only the DATA PLANE (URL resolution + a DuckDB executor), the same
 * shape as `@fossil-lang/graph`'s injected `DuckExecutor`.
 *
 * Framework-agnostic + zero @fossil-lang deps (a true leaf). React glue +
 * the descriptor→LSP-worker push live in `@fossil-lang/editor`; the host
 * decides how `resolve`/`query` reach its cloud + DuckDB.
 *
 * The primitive union below is the wire form of `fossil-graph-schema`'s
 * `Primitive`; the DuckDB mapping mirrors the Rust sibling
 * `duckdb_type_to_fossil_primitive` in `fossil-engine`. A value outside the
 * union is rejected when the descriptor is registered.
 */

/**
 * The Fossil primitive lattice (mirror `@fossil-lang/wasm`'s
 * `InferredPrimitive` — structurally identical so `introspect()` output flows
 * straight into `FossilPlayground.registerInferredDescriptor`). Consolidating
 * the single source of these types is a follow-up (see EDITOR-SCHEMA-AWARE-PLAN
 * D-2).
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
  /** The source binding name (`users` from `users := io.csv(...)`). */
  source_name: string;
  /** Ordered, position-significant columns. */
  columns: InferredColumn[];
  /** Empty from the host; the Rust side derives it (ADR-0037). */
  content_hash: string;
}

/** A source binding scraped from a `.fossil` mapping. */
export interface SourceRef {
  sourceName: string;
  url: string;
}

/** A single row from DuckDB's `DESCRIBE SELECT * FROM read_csv_auto(...)`. */
export interface DescribeRow {
  column_name?: unknown;
  column_type?: unknown;
}

/**
 * Map a DuckDB column-type string onto the Fossil lattice. MUST match the Rust
 * sibling `duckdb_type_to_fossil_primitive` in `fossil-engine`.
 */
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
  // VARCHAR / TEXT / STRING + any unrecognised type fall back to String
  // (matching the fossil-hir wildcard arm).
  return "string";
}

/**
 * Scrape source-binding RHS URLs from a `.fossil` text. Mirrors the Rust
 * sibling `extract_source_refs` (crates/fossil-cli/src/main.rs).
 *
 * LIMITATIONS (regex placeholder; an AST walk supersedes it): no multi-line
 * constructor, no interleaved comments between `:=` and `io.csv(`, no
 * backslash-escaped quotes inside the URL string.
 */
export function extractSourceRefs(text: string): SourceRef[] {
  const re = /(\w[\w\d_]*)\s*:=\s*io\.(?:csv|json)\(\s*['"]([^'"]+)['"]/g;
  const out: SourceRef[] = [];
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m[1] && m[2]) {
      out.push({ sourceName: m[1], url: m[2] });
    }
  }
  return out;
}

/**
 * The canonical DESCRIBE SQL for a resolved source URL. `read_csv_auto` is
 * single-quote-escaped (a SQL string literal, not a prepared parameter).
 *
 * NOTE: matches the fossil-cli + playground reference, which uses
 * `read_csv_auto` for both csv and json refs today; a json-aware variant is a
 * cross-home change (must land in all impls at once to preserve parity).
 */
export function describeSql(url: string): string {
  const escaped = url.replace(/'/g, "''");
  return `DESCRIBE SELECT * FROM read_csv_auto('${escaped}')`;
}

/**
 * Build the descriptor a `DESCRIBE` produced for one source binding. Columns
 * with empty/missing names are dropped (defensive against malformed rows).
 */
export function buildDescriptor(
  sourceName: string,
  describeRows: readonly DescribeRow[],
): InferredDescriptor {
  const columns: InferredColumn[] = describeRows
    .map((r) => ({
      name: String(r.column_name ?? ""),
      primitive: duckdbTypeToFossilPrimitive(String(r.column_type ?? "")),
    }))
    .filter((c) => c.name.length > 0);
  return { source_name: sourceName, columns, content_hash: "" };
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
      const rows = await io.query(describeSql(url));
      out.push(buildDescriptor(ref.sourceName, rows));
    } catch (err) {
      warn(
        `[introspect] source \`${ref.sourceName}\` (url=\`${ref.url}\`) failed`,
        err,
      );
    }
  }
  return out;
}
