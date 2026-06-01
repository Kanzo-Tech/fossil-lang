/**
 * Phase 13 v0.2 (ADR-0037) — host-side `InferredDescriptor` orchestration.
 *
 * Now a thin React adapter over `@fossil-lang/introspect` (the canonical home;
 * see .planning/EDITOR-SCHEMA-AWARE-PLAN.md). The package owns the parsing +
 * DuckDB→Fossil type table + DESCRIBE SQL + descriptor shape; this hook injects
 * the playground's data plane — the `ConnectionResolver` (→ URL) and a
 * DuckDB-WASM connection (→ DESCRIBE rows) — then registers the resulting
 * descriptors on the main-thread `FossilPlayground` instance + forwards each to
 * the LSP worker via the `onDescriptor` callback.
 *
 * Called BEFORE every `compile()` / `compileFile()` invocation. Failures per
 * source are non-fatal (logged + skipped by `introspect`).
 */

import { useCallback } from 'react';
import { parseSourceRef } from '@fossil-lang/resolvers';
import { introspect } from '@fossil-lang/introspect';
import type { FossilPlayground, InferredDescriptorJson } from '@fossil-lang/wasm';
import type { ConnectionResolver } from '@fossil-lang/types';
import type { SourceSchema } from '../component/SourcePanel.js';

// Re-export the pure helpers from the canonical package so existing imports
// (`run/` barrel + the vitest spec that imports them from this hook) survive.
export {
  extractSourceRefs,
  duckdbTypeToFossilPrimitive,
} from '@fossil-lang/introspect';

import { extractSourceRefs } from '@fossil-lang/introspect';

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
   * `playground`. Forwards each descriptor to `onDescriptor` (the LSP-worker
   * push). Resolves to the captured `SourceSchema[]` (empty when there are no
   * refs or the DuckDB connection failed; partial when individual sources
   * fail). Per-source failures are non-fatal (logged + skipped).
   */
  introspectAndRegister(
    mappingText: string,
    playground: FossilPlayground,
    onDescriptor?: (descriptor: InferredDescriptorJson) => void,
  ): Promise<SourceSchema[]>;
}

/**
 * React hook returning a stable `introspectAndRegister` callback the playground
 * awaits BEFORE invoking WASM `compile()`.
 */
export function useInferredDescriptors(
  args: UseInferredDescriptorsArgs,
): InferredDescriptorsApi {
  const { resolver, connectionFactory } = args;

  const introspectAndRegister = useCallback(
    async (
      mappingText: string,
      playground: FossilPlayground,
      onDescriptor?: (descriptor: InferredDescriptorJson) => void,
    ): Promise<SourceSchema[]> => {
      // Guard: do not open a DuckDB connection when there's nothing to read.
      if (extractSourceRefs(mappingText).length === 0) return [];

      let conn: DescribingConnection | null = null;
      try {
        conn = await connectionFactory();
      } catch (err) {
        // DuckDB-WASM didn't boot — non-fatal; skip pre-introspection.
        // eslint-disable-next-line no-console
        console.warn(
          '[useInferredDescriptors] DuckDB connection failed; skipping pre-introspection:',
          err,
        );
        return [];
      }

      try {
        // The package owns parsing + DESCRIBE + type-mapping; we inject the
        // playground's data plane (ConnectionResolver + this DuckDB connection).
        const descriptors = await introspect(mappingText, {
          resolve: async (ref) => {
            const resolved = await resolver.resolve(parseSourceRef(ref.url));
            return resolved.url;
          },
          query: async (sql) => (await conn!.query(sql)).toArray(),
          // eslint-disable-next-line no-console
          onWarn: (message, err) => console.warn(message, err),
        });

        const captured: SourceSchema[] = [];
        for (const descriptor of descriptors) {
          playground.registerInferredDescriptor(descriptor);
          // Push the SAME descriptor to the LSP worker's FossilPlayground (the
          // worker instance is distinct from this main-thread one, ADR-0026) so
          // editor source-field completion sees the columns.
          onDescriptor?.(descriptor);
          captured.push({
            sourceName: descriptor.source_name,
            columns: descriptor.columns.map((c) => ({
              name: c.name,
              primitive: c.primitive,
            })),
          });
        }
        return captured;
      } finally {
        try {
          await conn.close();
        } catch {
          // Swallow — connection-failure path already logged if any.
        }
      }
    },
    [resolver, connectionFactory],
  );

  return { introspectAndRegister };
}
