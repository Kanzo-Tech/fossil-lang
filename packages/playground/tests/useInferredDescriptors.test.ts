/**
 * Vitest coverage for useInferredDescriptors (plan 13-04b, ADR-0037).
 *
 * Exercises the pure functions (`extractSourceRefs` +
 * `duckdbTypeToFossilPrimitive`) directly and the hook's
 * `introspectAndRegister` callback against a mock FossilPlayground + mock
 * DuckDB connection.
 *
 * The documented regex limitations (multi-line constructor, comment between
 * `:=` and `io.csv(`, backslash-escaped quotes) are asserted with
 * `it.skip` + a comment so future contributors understand the v0.2
 * placeholder design.
 */

import { describe, it, expect, vi } from 'vitest';
import {
  extractSourceRefs,
  duckdbTypeToFossilPrimitive,
  useInferredDescriptors,
  type DescribingConnection,
} from '../src/hooks/useInferredDescriptors.js';
import { renderHook, act } from '@testing-library/react';
import type { FossilPlayground, InferredDescriptorJson } from '@fossil-lang/wasm';
import type { ConnectionResolver, ResolvedSource } from '@fossil-lang/types';

// ----- extractSourceRefs -----

describe('extractSourceRefs', () => {
  it('captures a single io.csv("...") binding', () => {
    const src = 'users := io.csv("u.csv")\n';
    expect(extractSourceRefs(src)).toEqual([{ sourceName: 'users', url: 'u.csv' }]);
  });

  it('captures multiple bindings (csv + json)', () => {
    const src = `users := io.csv("u.csv")\norders := io.json("o.json")\n`;
    expect(extractSourceRefs(src)).toEqual([
      { sourceName: 'users', url: 'u.csv' },
      { sourceName: 'orders', url: 'o.json' },
    ]);
  });

  it('accepts single-quoted URLs', () => {
    const src = "users := io.csv('u.csv')";
    expect(extractSourceRefs(src)).toEqual([{ sourceName: 'users', url: 'u.csv' }]);
  });

  it('handles whitespace between := and io.csv(', () => {
    const src = 'users   :=   io.csv(   "u.csv"   )';
    expect(extractSourceRefs(src)).toEqual([{ sourceName: 'users', url: 'u.csv' }]);
  });

  it('returns empty array on a no-source file', () => {
    expect(extractSourceRefs('prefix ex: <https://example.org/>\n')).toEqual([]);
  });

  // ---- Documented limitations (v0.2 placeholder; Phase 14+ AST walk) ----

  it.skip('LIMITATION: multi-line constructor — name :=\\n  io.csv("...")', () => {
    // Phase 14+ will replace the regex with an AST walk that handles
    // multi-line forms. Documented here so the gap is visible.
    const src = 'users :=\n  io.csv("u.csv")\n';
    expect(extractSourceRefs(src)).toEqual([{ sourceName: 'users', url: 'u.csv' }]);
  });

  it.skip('LIMITATION: comment between := and io.csv(', () => {
    const src = 'users := // a comment\n  io.csv("u.csv")';
    expect(extractSourceRefs(src)).toEqual([{ sourceName: 'users', url: 'u.csv' }]);
  });

  it.skip('LIMITATION: backslash-escaped quotes inside the URL string', () => {
    // `io.csv("u\"esc.csv")` is malformed for the v0.2 regex (the inner `"`
    // terminates the URL capture). v0.2 callers should not use escaped quotes
    // in URLs — DuckDB also doesn't support them inside SQL string literals
    // without similar gymnastics.
    const src = 'users := io.csv("u\\"esc.csv")';
    expect(extractSourceRefs(src)).toEqual([{ sourceName: 'users', url: 'u\\"esc.csv' }]);
  });
});

// ----- duckdbTypeToFossilPrimitive -----

describe('duckdbTypeToFossilPrimitive', () => {
  it('maps integer-family types to Integer', () => {
    for (const t of ['INTEGER', 'BIGINT', 'INT', 'SMALLINT', 'TINYINT', 'HUGEINT']) {
      expect(duckdbTypeToFossilPrimitive(t)).toBe('Integer');
    }
  });

  it('maps float-family types to Float (including DECIMAL(P,S))', () => {
    expect(duckdbTypeToFossilPrimitive('DOUBLE')).toBe('Float');
    expect(duckdbTypeToFossilPrimitive('FLOAT')).toBe('Float');
    expect(duckdbTypeToFossilPrimitive('REAL')).toBe('Float');
    expect(duckdbTypeToFossilPrimitive('DECIMAL(10,2)')).toBe('Float');
    expect(duckdbTypeToFossilPrimitive('DECIMAL')).toBe('Float');
  });

  it('maps BOOLEAN/BOOL to Bool', () => {
    expect(duckdbTypeToFossilPrimitive('BOOLEAN')).toBe('Bool');
    expect(duckdbTypeToFossilPrimitive('BOOL')).toBe('Bool');
  });

  it('maps date/time-family types', () => {
    expect(duckdbTypeToFossilPrimitive('DATE')).toBe('Date');
    expect(duckdbTypeToFossilPrimitive('TIMESTAMP')).toBe('DateTime');
    expect(duckdbTypeToFossilPrimitive('DATETIME')).toBe('DateTime');
    expect(duckdbTypeToFossilPrimitive('TIME')).toBe('Time');
  });

  it('defaults VARCHAR / TEXT / STRING and any unrecognised type to String', () => {
    expect(duckdbTypeToFossilPrimitive('VARCHAR')).toBe('String');
    expect(duckdbTypeToFossilPrimitive('TEXT')).toBe('String');
    expect(duckdbTypeToFossilPrimitive('STRING')).toBe('String');
    expect(duckdbTypeToFossilPrimitive('UNKNOWN_TYPE')).toBe('String');
    expect(duckdbTypeToFossilPrimitive('')).toBe('String');
  });

  it('is case-insensitive and tolerates whitespace', () => {
    expect(duckdbTypeToFossilPrimitive(' integer ')).toBe('Integer');
    expect(duckdbTypeToFossilPrimitive('Bigint')).toBe('Integer');
  });
});

// ----- introspectAndRegister against mocks -----

function makeMockConnection(
  rows: Array<{ column_name: string; column_type: string }>,
): DescribingConnection & { closed: boolean; lastSql: string | null } {
  const self = {
    closed: false,
    lastSql: null as string | null,
    async query(sql: string) {
      self.lastSql = sql;
      return { toArray: () => rows };
    },
    async close() {
      self.closed = true;
    },
  };
  return self;
}

function makeMockResolver(): ConnectionResolver {
  return {
    async resolve(): Promise<ResolvedSource> {
      return { url: 'https://playground-public.kanzo.dev/u.csv', format: 'csv' };
    },
  } as ConnectionResolver;
}

function makeMockPlayground(): Pick<FossilPlayground, 'registerInferredDescriptor'> & {
  calls: InferredDescriptorJson[];
} {
  const calls: InferredDescriptorJson[] = [];
  return {
    calls,
    registerInferredDescriptor(d: InferredDescriptorJson) {
      calls.push(d);
    },
  };
}

describe('useInferredDescriptors.introspectAndRegister', () => {
  it('registers an InferredDescriptor for a single io.csv binding', async () => {
    const conn = makeMockConnection([
      { column_name: 'id', column_type: 'INTEGER' },
      { column_name: 'name', column_type: 'VARCHAR' },
    ]);
    const pg = makeMockPlayground();
    const resolver = makeMockResolver();
    const { result } = renderHook(() =>
      useInferredDescriptors({
        resolver,
        connectionFactory: async () => conn,
      }),
    );
    let returned: unknown;
    await act(async () => {
      returned = await result.current.introspectAndRegister(
        'users := io.csv("@examples/u.csv")',
        pg as unknown as FossilPlayground,
      );
    });
    expect(pg.calls).toHaveLength(1);
    expect(pg.calls[0]).toEqual({
      source_name: 'users',
      columns: [
        { name: 'id', primitive: 'Integer' },
        { name: 'name', primitive: 'String' },
      ],
      content_hash: '',
    });
    // Phase 14 plan 14-03: the hook now also returns the captured schemas
    // (`Promise<SourceSchema[]>`) so the SourcePanel can render the preview
    // without a second DESCRIBE pass.
    expect(returned).toEqual([
      {
        sourceName: 'users',
        columns: [
          { name: 'id', primitive: 'Integer' },
          { name: 'name', primitive: 'String' },
        ],
      },
    ]);
    expect(conn.closed).toBe(true);
  });

  it('skips when there are no source refs (returns empty SourceSchema[])', async () => {
    const conn = makeMockConnection([]);
    const pg = makeMockPlayground();
    const resolver = makeMockResolver();
    const factory = vi.fn(async () => conn);
    const { result } = renderHook(() =>
      useInferredDescriptors({ resolver, connectionFactory: factory }),
    );
    let returned: unknown;
    await act(async () => {
      returned = await result.current.introspectAndRegister(
        'prefix ex: <https://example.org/>\n',
        pg as unknown as FossilPlayground,
      );
    });
    expect(pg.calls).toHaveLength(0);
    expect(factory).not.toHaveBeenCalled();
    expect(returned).toEqual([]);
  });

  it('continues past a per-source failure (logs + skips)', async () => {
    const conn: DescribingConnection = {
      async query() {
        throw new Error('DESCRIBE failed');
      },
      async close() {},
    };
    const pg = makeMockPlayground();
    const resolver = makeMockResolver();
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { result } = renderHook(() =>
      useInferredDescriptors({
        resolver,
        connectionFactory: async () => conn,
      }),
    );
    await act(async () => {
      await result.current.introspectAndRegister(
        'users := io.csv("@examples/u.csv")\norders := io.json("@examples/o.json")',
        pg as unknown as FossilPlayground,
      );
    });
    expect(pg.calls).toHaveLength(0);
    expect(warnSpy).toHaveBeenCalled();
    warnSpy.mockRestore();
  });
});
