/**
 * Source-binding introspection helpers — pure functions that the Phase 13
 * host-side `InferredDescriptor` flow (ADR-0037 / plan 13-04b) leans on:
 *
 *   - `extractSourceRefs(text)` — scrape `io.csv("...")` / `io.json("...")`
 *     source-binding RHS URLs from a `.fossil` mapping text.
 *   - `duckdbTypeToFossilPrimitive(t)` — map a DuckDB column-type string to
 *     the canonical Fossil `Primitive` name.
 *
 * These were previously inlined in `hooks/useInferredDescriptors.ts`; Phase
 * 14 plan 14-02 moves them into the `run/` namespace per CONTEXT.md COMP-02
 * ("run/ — runPipeline + transformSql + introspection") so the layer is
 * organised by function (compute helpers in `run/`; React hooks in `hooks/`).
 * The hook keeps a transitive re-export so test imports survive byte-for-byte.
 *
 * Both helpers mirror Rust siblings in `crates/fossil-cli/src/main.rs`
 * (`extract_source_refs` + `duckdb_type_to_fossil_primitive`) — the playground
 * pre-introspection step + the CLI's `pre_introspect_and_register` MUST agree
 * on the same source-name → primitive mapping.
 *
 * LIMITATIONS (documented; Phase 14+ replaces with an AST walk):
 *   - does NOT match multi-line constructor (`name :=\n  io.csv("...")`)
 *   - does NOT match interleaved comments between `:=` and `io.csv(`
 *   - does NOT handle backslash-escaped quotes inside the URL string
 */

/**
 * Map a `DuckDB` column-type string to the canonical Fossil `Primitive`
 * name. MUST match `fossil-hir::infer::primitive_from_name`'s table
 * (canonical names: `Integer`, `Float`, `String`, `Bool`, `Date`,
 * `DateTime`, `Time`, `GYear`, `AnyURI`).
 */
export function duckdbTypeToFossilPrimitive(t: string): string {
  const upper = t.trim().toUpperCase();
  if (
    upper === 'INTEGER' ||
    upper === 'BIGINT' ||
    upper === 'INT' ||
    upper === 'SMALLINT' ||
    upper === 'TINYINT' ||
    upper === 'HUGEINT'
  ) {
    return 'Integer';
  }
  if (upper === 'DOUBLE' || upper === 'FLOAT' || upper === 'REAL') return 'Float';
  if (upper.startsWith('DECIMAL')) return 'Float';
  if (upper === 'BOOLEAN' || upper === 'BOOL') return 'Bool';
  if (upper === 'DATE') return 'Date';
  if (upper === 'TIMESTAMP' || upper === 'DATETIME') return 'DateTime';
  if (upper === 'TIME') return 'Time';
  // VARCHAR / TEXT / STRING + any unrecognised type fall back to String
  // (matching the fossil-hir wildcard arm).
  return 'String';
}

/**
 * Scrape source-binding RHS source URLs from a `.fossil` text.
 *
 * Regex-based (v0.2 placeholder). Shape mirrors the Rust sibling
 * `extract_source_refs` in `crates/fossil-cli/src/main.rs` so playground
 * + CLI behave identically.
 */
export function extractSourceRefs(
  text: string,
): Array<{ sourceName: string; url: string }> {
  const re = /(\w[\w\d_]*)\s*:=\s*io\.(?:csv|json)\(\s*['"]([^'"]+)['"]/g;
  const out: Array<{ sourceName: string; url: string }> = [];
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m[1] && m[2]) {
      out.push({ sourceName: m[1], url: m[2] });
    }
  }
  return out;
}
