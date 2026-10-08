/**
 * The DESCRIBE a host runs for one source, and the descriptor it builds from the answer.
 */
import type { InferredColumn, InferredDescriptor } from "@fossil-lang/types";

import { NATIVE_OPTIONS, NATIVE_READERS, duckdbPrimitive, type NativeRow } from "./catalogue.generated.js";

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
  format: NativeRow,
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
 * `etag` is what the host knows about the source's state — an ETag or a
 * `Last-Modified` is the cheap one. Omitted, it is `""`: never fresh, so the compiler re-introspects
 * every time. That is the correct default for a host that has not wired one,
 * and it is not a hash of the columns — a token derived from the answer cannot
 * tell you whether to ask the question.
 */
export function buildDescriptor(
  key: string,
  describeRows: readonly DescribeRow[],
  etag = "",
): InferredDescriptor {
  const columns: InferredColumn[] = describeRows
    .map((r) => ({
      name: String(r.column_name ?? ""),
      primitive: duckdbPrimitive(String(r.column_type ?? "")),
    }))
    .filter((c) => c.name.length > 0);
  return { key, columns, etag };
}
