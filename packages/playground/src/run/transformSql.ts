/**
 * SQL transformer for the playground Run pipeline.
 *
 * Pure functions, dependency-injected resolver — same testability the rest of
 * the package enjoys (see `useDuckDb`, `useResetPlayground`). Implements the
 * three transforms the codegen-emitted SQL needs before it can run inside
 * DuckDB-WASM:
 *
 *   1. Extract every `@connector/path` literal from `read_csv_auto(...)` /
 *      `read_json_auto(...)` / `read_parquet(...)` calls so the caller can
 *      drive the {@link ConnectionResolver}.
 *   2. Substitute each ref's resolved URL into the SQL string in-place,
 *      enforcing a Content-Length cap BEFORE the bytes ever reach DuckDB
 *      (PLAY-12 — defence in depth against the ~50MB OOM scenario).
 *   3. Rewrite every `COPY (<inner>) TO '<prefix>.parquet' (FORMAT PARQUET);`
 *      into a `CREATE OR REPLACE TABLE "<name>" AS <inner>;` (first chunk of
 *      a given prefix) or `INSERT INTO "<name>" <inner>;` (subsequent chunks)
 *      so the rows survive the run inside DuckDB's catalog — DuckDB-WASM
 *      can't (and need not) write parquet to OPFS for the playground.
 *
 * # SQL shapes the codegen emits
 *
 * The codegen produces two shapes (see `crates/fossil-codegen/src/sql.rs`):
 *
 * **AcceptAll** (the walking-skeleton, `hello.fossil` default — no ShEx target):
 *   ```sql
 *   CREATE VIEW users AS
 *   SELECT * FROM read_csv_auto('@examples/hello.csv', sample_size=-1);
 *   COPY (
 *       SELECT 'https://example.org/user/' || ... AS subject,
 *              'https://example.org/name' AS predicate,
 *              users.name AS object
 *       FROM users
 *   ) TO 'output.parquet' (FORMAT PARQUET);
 *   ```
 *   ONE flat-triple COPY with `(subject, predicate, object)` columns; target
 *   path is the bare `output.parquet` (no `chunkN` suffix).
 *
 * **ShEx descriptor** (post-`set_target_shex`):
 *   ```sql
 *   CREATE VIEW people AS ...;
 *   COPY (...) TO 'vertex/person/chunk0.parquet' (FORMAT PARQUET);
 *   COPY (...) TO 'edge/person_knows_person/chunk0.parquet' (FORMAT PARQUET);
 *   ```
 *   Per-vertex/per-edge COPYs with `id`/`src_id`/`dst_id` column conventions.
 *
 * Both shapes must round-trip through {@link transformSql} into runnable SQL.
 */

import type { ConnectionResolver, SourceRef } from '@fossil-lang/types';
import { parseSourceRef } from '@fossil-lang/resolvers';

/**
 * Regex matching `read_csv_auto('...')` / `read_csv('...')` /
 * `read_json_auto('...')` / `read_json('...')` / `read_parquet('...')`. The
 * capture group is the single-quoted URI literal — the candidate
 * `@connector/path` reference (or a literal URL, which we leave alone).
 *
 * `[^']+` is the URI body. We deliberately accept any non-quote content (URLs
 * can contain `/`, `?`, `&`, etc.); the parse-or-skip path downstream guards
 * against malformed `@`-refs.
 */
const SOURCE_READER_REGEX =
  /(read_csv_auto|read_csv|read_json_auto|read_json|read_parquet)\s*\(\s*'([^']+)'/g;

/**
 * Regex matching one `COPY (<inner>) TO '<path>' (FORMAT PARQUET);` statement.
 * Capture 1 = the inner SELECT body (without the outer parens). Capture 2 =
 * the target path literal. The `[\s\S]*?` lazy match for the inner body lets
 * us span the multi-line SELECTs the codegen emits without choking on
 * embedded `)` in window functions or CASE expressions — we anchor on
 * `) TO '...'` which the codegen always emits verbatim.
 *
 * The `[\s;]*` tail eats the trailing semicolon + any whitespace so the
 * rewriter doesn't leave stranded semicolons in the output string.
 */
const COPY_TO_PARQUET_REGEX =
  /COPY\s*\(\s*([\s\S]*?)\s*\)\s*TO\s*'([^']+)'\s*\(FORMAT\s+PARQUET\)\s*;?/gi;

/**
 * Classification of one rewritten COPY target.
 *
 *  - `vertex` — path matched `vertex/<name>/chunkN.parquet`; the readback
 *    SELECTs the table as a vertex row (project `iri`/`id` → `id`).
 *  - `edge`   — path matched `edge/<name>/chunkN.parquet`; readback projects
 *    `src_id`/`dst_id` → `source`/`target`.
 *  - `triple` — anything else (notably the AcceptAll `output.parquet`); the
 *    readback fans the table out as BOTH vertices (distinct subjects) AND
 *    edges (subject→source, object→target, predicate kept verbatim) so the
 *    user sees something for the walking-skeleton case.
 */
export type TableClass = 'vertex' | 'edge' | 'triple';

/**
 * Extract every `@connector/path` reference inside `read_*` source-reader
 * calls in `sql`. Literals that don't start with `@` (already-resolved URLs,
 * future literal paths) are skipped. Literals that start with `@` but fail
 * {@link parseSourceRef} (invalid connector name, missing `/`) are ALSO
 * skipped silently — they're not Resolver targets, so handing them to the
 * resolver would only produce noise. The caller can detect "no refs found"
 * and short-circuit the resolver loop entirely.
 */
export function extractSourceRefs(
  sql: string,
): Array<{ ref: SourceRef; literal: string }> {
  const out: Array<{ ref: SourceRef; literal: string }> = [];
  // Reset the regex's lastIndex since it's module-scope and stateful (`/g`).
  SOURCE_READER_REGEX.lastIndex = 0;
  for (const match of sql.matchAll(SOURCE_READER_REGEX)) {
    const literal = match[2];
    if (!literal || !literal.startsWith('@')) continue;
    try {
      const ref = parseSourceRef(literal);
      out.push({ ref, literal });
    } catch {
      // Not a well-formed SourceRef — leave it alone. The resolver isn't
      // responsible for arbitrary `@`-prefixed text the user typed.
    }
  }
  return out;
}

/**
 * Derive a SQL identifier + classification from a `COPY ... TO '<path>'`
 * target path. Examples:
 *
 *   `vertex/Person/chunk0.parquet`         → { name: 'Person',                 class: 'vertex' }
 *   `vertex/person/chunk0.parquet`         → { name: 'person',                 class: 'vertex' }
 *   `edge/Person_knows_Person/chunk0.parquet` → { name: 'Person_knows_Person', class: 'edge' }
 *   `output.parquet`                       → { name: 'output',                 class: 'triple' }
 *   `triples/chunk0.parquet`               → { name: 'triples',                class: 'triple' }
 *
 * The chunk number is parsed out separately — multiple chunks of the same
 * prefix collapse to ONE `CREATE TABLE` + N-1 `INSERT INTO`.
 */
function classifyCopyTarget(path: string): {
  name: string;
  class: TableClass;
  chunk: number;
} {
  // Bare `output.parquet` (AcceptAll) — no chunk suffix.
  // Also any `<name>.parquet` without an interior `/chunk<k>.parquet`.
  const chunkMatch = /^(.+?)\/chunk(\d+)\.parquet$/.exec(path);
  if (!chunkMatch) {
    // Strip the trailing `.parquet` and treat as a single-chunk table.
    const bare = path.replace(/\.parquet$/, '');
    // Sanitise any path separators a future codegen path might emit.
    const name = bare.replace(/[^A-Za-z0-9_]/g, '_');
    // AcceptAll's `output.parquet` is flat triples — classify as 'triple' so
    // the readback knows to project subject/object.
    return { name, class: 'triple', chunk: 0 };
  }
  const prefix = chunkMatch[1]!;
  const chunk = Number(chunkMatch[2]);
  // Split the prefix; the first segment is the class hint.
  const firstSlash = prefix.indexOf('/');
  const firstSegment = firstSlash < 0 ? prefix : prefix.slice(0, firstSlash);
  const rest = firstSlash < 0 ? '' : prefix.slice(firstSlash + 1);
  let cls: TableClass = 'triple';
  let nameRaw: string;
  if (firstSegment === 'vertex' && rest.length > 0) {
    cls = 'vertex';
    nameRaw = rest;
  } else if (firstSegment === 'edge' && rest.length > 0) {
    cls = 'edge';
    nameRaw = rest;
  } else {
    // Unknown prefix (e.g. `triples/chunk0.parquet`) — keep the whole
    // prefix as the name and classify as triple.
    nameRaw = prefix;
  }
  const name = nameRaw.replace(/[^A-Za-z0-9_]/g, '_');
  return { name, class: cls, chunk };
}

/**
 * Rewrite every `COPY (<inner>) TO '<path>.parquet' (FORMAT PARQUET);` in
 * `sql` into a DuckDB-WASM-compatible `CREATE OR REPLACE TABLE` / `INSERT
 * INTO` pair. Per-prefix collapse: the first chunk creates the table; every
 * subsequent chunk of the same prefix appends via INSERT, preserving the
 * row union the GraphAr decomposition relied on.
 *
 * Returns the rewritten SQL plus three disjoint table lists keyed by prefix
 * class so the runPipeline knows what to SELECT FROM afterward. Each list
 * preserves first-seen order — the caller iterates and projects rows in
 * the order the COPYs were emitted.
 */
export function rewriteCopyToCreateTable(sql: string): {
  rewrittenSql: string;
  vertexTables: string[];
  edgeTables: string[];
  tripleTables: string[];
} {
  const vertexTables: string[] = [];
  const edgeTables: string[] = [];
  const tripleTables: string[] = [];
  // Track first-seen per table name so chunk0 → CREATE, chunkN>0 → INSERT.
  const seen = new Set<string>();
  // Reset lastIndex (module-scope `/g` regex is stateful).
  COPY_TO_PARQUET_REGEX.lastIndex = 0;
  const rewrittenSql = sql.replace(
    COPY_TO_PARQUET_REGEX,
    (_match, innerRaw: string, path: string) => {
      const inner = innerRaw.trim();
      const { name, class: cls } = classifyCopyTarget(path);
      // Classify into the right output bucket on FIRST sight only — the
      // runPipeline iterates the list once and does one SELECT per name.
      if (!seen.has(name)) {
        seen.add(name);
        if (cls === 'vertex') vertexTables.push(name);
        else if (cls === 'edge') edgeTables.push(name);
        else tripleTables.push(name);
        return `CREATE OR REPLACE TABLE "${name}" AS ${inner};`;
      }
      // Subsequent chunks of the same prefix — append rows to the same
      // table. DuckDB's INSERT INTO accepts a bare SELECT.
      return `INSERT INTO "${name}" ${inner};`;
    },
  );
  return { rewrittenSql, vertexTables, edgeTables, tripleTables };
}

/**
 * The Content-Length cap check (PLAY-12). For each resolved URL, HEAD-fetch
 * to learn the byte size; refuse if it exceeds `maxResolvedBytes`. Some CDNs
 * strip Content-Length on HEAD; we treat missing headers as "size unknown"
 * and let the request through (the cap is defence in depth, not a security
 * boundary — the resolver itself owns the credential / authorisation story).
 *
 * `maxResolvedBytes <= 0` disables the cap (per the FossilPlaygroundProps
 * contract — set 0 to opt out, NOT recommended for production).
 *
 * Errors are thrown with the source ref's raw text so the user can correlate
 * the failure to the specific `@connector/path` in their mapping.
 */
async function enforceContentLengthCap(
  url: string,
  ref: SourceRef,
  maxResolvedBytes: number,
): Promise<void> {
  if (maxResolvedBytes <= 0) return;
  let response: Response;
  try {
    response = await fetch(url, { method: 'HEAD' });
  } catch {
    // HEAD might be unsupported (some CORS configurations); silently pass.
    return;
  }
  const lengthHeader = response.headers.get('content-length');
  if (lengthHeader === null) return;
  const size = Number(lengthHeader);
  if (!Number.isFinite(size)) return;
  if (size > maxResolvedBytes) {
    throw new Error(
      `Resolved source exceeds maxResolvedBytes cap (${size} > ${maxResolvedBytes} bytes): ${ref.raw}`,
    );
  }
}

/**
 * Escape a string for SQL single-quote literal context. The codegen always
 * wraps source URIs in single-quoted literals; the replacement URL goes back
 * into the same slot, so we must escape any literal single quotes (DuckDB's
 * convention: double them, `it's` → `it''s`).
 *
 * `blob:` URLs never contain quotes in practice (the spec doesn't permit
 * them in the opaque part); `https:` URLs likewise don't carry quotes in
 * well-formed paths. This escape is defence in depth for hypothetical Tier-2
 * resolvers that return signed URLs with unusual encodings.
 */
function escapeSqlLiteral(s: string): string {
  return s.replace(/'/g, "''");
}

/**
 * Result of the full SQL transform — what the runPipeline hands to DuckDB.
 *
 * `registeredFiles` is the list of (virtual-name, resolved-URL) pairs the
 * runPipeline should register with DuckDB-WASM via `registerFileURL` BEFORE
 * executing the SQL. For Tier-1 `blob:` URLs registering them under their
 * `@connector/path` virtual name keeps `read_csv_auto('@examples/...')`
 * working even though the SQL has been substituted in-place — registering
 * is belt-AND-braces (it lets DuckDB's HTTP filesystem reach the blob URL
 * via its native HTTP protocol).
 */
export interface TransformedSql {
  /** SQL with URL substitutions + COPY-to-CREATE-TABLE rewrites applied. */
  executableSql: string;
  /** Vertex table names (rewriter classification) — in COPY-order. */
  vertexTables: string[];
  /** Edge table names — in COPY-order. */
  edgeTables: string[];
  /** Flat-triple / unknown-prefix table names — in COPY-order. */
  tripleTables: string[];
  /** Files the caller should register with DuckDB-WASM before execution. */
  registeredFiles: Array<{ virtualName: string; url: string }>;
}

/**
 * Full SQL transform: resolve every `@connector/path` reference, substitute
 * the URLs in, then rewrite COPYs into CREATE TABLEs.
 *
 *   sql → extractSourceRefs → for ref in refs: resolver.resolve(ref) →
 *         enforceContentLengthCap(url, ref) → substitute literal → rewriteCopyToCreateTable
 *
 * Substitution is in-place via simple string replace — the candidate literal
 * is unique per ref (it's the user-typed `@connector/path` text), so a
 * `split(literal).join(url)` collapses all occurrences in one pass without
 * regex-escaping the replacement string.
 */
export async function transformSql(
  sql: string,
  resolver: ConnectionResolver,
  opts: { maxResolvedBytes: number },
): Promise<TransformedSql> {
  const refs = extractSourceRefs(sql);
  const registeredFiles: Array<{ virtualName: string; url: string }> = [];
  let working = sql;
  // De-duplicate refs by literal — the same `@examples/hello.csv` may appear
  // in N source readers; one resolver call serves all.
  const seenLiterals = new Set<string>();
  for (const { ref, literal } of refs) {
    if (seenLiterals.has(literal)) continue;
    seenLiterals.add(literal);
    const resolved = await resolver.resolve(ref);
    await enforceContentLengthCap(resolved.url, ref, opts.maxResolvedBytes);
    const escapedUrl = escapeSqlLiteral(resolved.url);
    // Substitute the literal text wherever it appears between the surrounding
    // single quotes. Because `literal` is the user-supplied connector/path
    // string (matched verbatim from the SQL), splitting on it preserves the
    // surrounding `'...'` quotes the codegen emitted.
    working = working.split(literal).join(escapedUrl);
    registeredFiles.push({ virtualName: literal, url: resolved.url });
  }
  const {
    rewrittenSql,
    vertexTables,
    edgeTables,
    tripleTables,
  } = rewriteCopyToCreateTable(working);
  return {
    executableSql: rewrittenSql,
    vertexTables,
    edgeTables,
    tripleTables,
    registeredFiles,
  };
}
