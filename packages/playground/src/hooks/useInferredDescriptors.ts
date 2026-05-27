/**
 * Phase 13 v0.2 (ADR-0037) — host-side `InferredDescriptor` orchestration.
 *
 * This hook is the TS sibling of `fossil-cli`'s `pre_introspect_and_register`
 * helper (plan 13-04a, `crates/fossil-cli/src/main.rs`):
 *
 *   1. Scrape `io.csv("...")` / `io.json("...")` source-binding references
 *      from the editor mapping text.
 *   2. For each ref, resolve the URL via the host's `ConnectionResolver`
 *      (Tier-1 bundled examples or Tier-2 host-mediated).
 *   3. Open a `DuckDB-WASM` connection + run
 *      `DESCRIBE read_csv_auto('<resolved-url>')`.
 *   4. Map each DuckDB column type to a canonical Fossil `Primitive` name
 *      (`Integer | Float | String | Bool | Date | DateTime | Time | GYear |
 *      AnyURI`) — the same table the Rust `fossil-hir::infer::primitive_from_name`
 *      lookup uses.
 *   5. Call `FossilPlayground.registerInferredDescriptor({source_name,
 *      columns, content_hash: ''})` via the `@fossil-lang/wasm` API
 *      (plan 13-03's `#[wasm_bindgen(js_name = registerInferredDescriptor)]`).
 *
 * Called BEFORE every `compile()` / `compileFile()` invocation. Replaces the
 * Phase 9 CSVW inference + editable-preview flow (plan 09-07's
 * `<CsvwPreview/>` + `inferCsvw` + `applyCsvw`).
 *
 * Failures per-source are non-fatal: log via `console.warn` + skip the
 * source. The compile may still succeed (legacy CSVW path if explicit
 * `schema=` arg present; otherwise no forward propagation for that
 * source).
 */

import { useCallback } from 'react';
import { parseSourceRef } from '@fossil-lang/resolvers';
import type { FossilPlayground, InferredDescriptorJson } from '@fossil-lang/wasm';
import type { ConnectionResolver } from '@fossil-lang/types';

// Phase 14 plan 14-02 (COMP-02): the pure source-binding introspection
// helpers (`extractSourceRefs` + `duckdbTypeToFossilPrimitive`) moved into
// the `run/` namespace per CONTEXT.md target layout. The hook keeps a
// transitive re-export so existing imports (incl. the vitest spec) survive
// byte-for-byte.
export {
  extractSourceRefs,
  duckdbTypeToFossilPrimitive,
} from '../run/introspection.js';

import {
  extractSourceRefs,
  duckdbTypeToFossilPrimitive,
} from '../run/introspection.js';

/** Minimal `DuckDB-WASM` connection shape this hook needs. Compatible with
 *  the async connection returned by `AsyncDuckDB.connect()`. */
export interface DescribingConnection {
  query(sql: string): Promise<{
    toArray(): Array<{ column_name?: unknown; column_type?: unknown }>;
  }>;
  close(): Promise<void>;
}

/** Factory for the per-call DuckDB connection. The playground passes a
 *  closure over its useDuckDb hook so the hook stays decoupled from the
 *  module's lifecycle layer. */
export type ConnectionFactory = () => Promise<DescribingConnection>;

export interface UseInferredDescriptorsArgs {
  /** The host-provided ConnectionResolver — already wired into the playground
   *  (Tier-1 default or Tier-2 host). */
  resolver: ConnectionResolver;
  /** Factory that returns a fresh DuckDB-WASM connection. The hook closes
   *  the connection it opens; the factory's underlying database stays alive
   *  across calls. */
  connectionFactory: ConnectionFactory;
}

export interface InferredDescriptorsApi {
  /**
   * Introspect every `io.csv("...")` / `io.json("...")` reference in
   * `mappingText` and register the resulting `InferredDescriptor` on
   * `playground` via `playground.registerInferredDescriptor(...)`.
   *
   * Failures per-source are non-fatal — log via `console.warn` + skip.
   * Resolves when ALL sources have been processed (registered or skipped).
   */
  introspectAndRegister(
    mappingText: string,
    playground: FossilPlayground,
  ): Promise<void>;
}

/**
 * React hook returning a stable `introspectAndRegister` callback that the
 * playground component awaits BEFORE invoking WASM `compile()`. Mirrors the
 * `fossil-cli` pre-introspection step (13-04a) so playground + CLI behave
 * identically on the same source.
 */
export function useInferredDescriptors(
  args: UseInferredDescriptorsArgs,
): InferredDescriptorsApi {
  const { resolver, connectionFactory } = args;

  const introspectAndRegister = useCallback(
    async (mappingText: string, playground: FossilPlayground): Promise<void> => {
      const refs = extractSourceRefs(mappingText);
      if (refs.length === 0) return;
      let conn: DescribingConnection | null = null;
      try {
        conn = await connectionFactory();
      } catch (err) {
        // DuckDB-WASM didn't boot — non-fatal (the legacy CSVW path may still
        // produce a working compile, or there's no forward-propagated
        // .field access on the source binding).
        // eslint-disable-next-line no-console
        console.warn(
          '[useInferredDescriptors] DuckDB connection failed; skipping pre-introspection:',
          err,
        );
        return;
      }
      try {
        for (const { sourceName, url } of refs) {
          try {
            const ref = parseSourceRef(url);
            const resolved = await resolver.resolve(ref);
            // Single-quote escape for the SQL literal — read_csv_auto takes a
            // SQL string, not a prepared-statement parameter.
            const escapedUrl = resolved.url.replace(/'/g, "''");
            const result = await conn.query(
              `DESCRIBE SELECT * FROM read_csv_auto('${escapedUrl}')`,
            );
            const rows = result.toArray();
            const columns: InferredDescriptorJson['columns'] = rows
              .map((r) => ({
                name: String(r.column_name ?? ''),
                primitive: duckdbTypeToFossilPrimitive(String(r.column_type ?? '')),
              }))
              .filter((c) => c.name.length > 0) as InferredDescriptorJson['columns'];
            const descriptor: InferredDescriptorJson = {
              source_name: sourceName,
              columns,
              content_hash: '',
            };
            playground.registerInferredDescriptor(descriptor);
          } catch (err) {
            // Per-source failure: log + skip (matches the Rust side's
            // tracing::warn convention).
            // eslint-disable-next-line no-console
            console.warn(
              `[useInferredDescriptors] introspection failed for source \`${sourceName}\` (url=\`${url}\`):`,
              err,
            );
          }
        }
      } finally {
        try {
          await conn.close();
        } catch {
          // Swallow — the connection failure path already logged if any.
        }
      }
    },
    [resolver, connectionFactory],
  );

  return { introspectAndRegister };
}
