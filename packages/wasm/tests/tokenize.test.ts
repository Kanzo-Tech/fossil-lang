/**
 * Node-side smoke test for @fossil-lang/wasm.
 *
 * Boots the wasm-bindgen --target web module in a Node Vitest run, then
 * exercises the JS-side public surface (tokenize, semanticLegend,
 * FossilPlayground class). The 08-02 cargo-test suite already covers the
 * Rust side (tokenize_native); this suite covers the JsValue → TS-shape
 * serialization boundary the Rust tests can't reach.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import {
  initFossilWasm,
  tokenize,
  semanticLegend,
  FossilPlayground,
} from '../src/index.js';
import type { TokenRow } from '@fossil-lang/types';

beforeAll(async () => {
  // wasm-bindgen --target web init() accepts URL/string/Request/Response/
  // BufferSource. In Node we can pass a file:// URL constructed from the
  // package layout, but `fetch` of `file://` URLs is environment-dependent
  // (Node 20+ accepts it via the WHATWG fetch global, Node 18 does not).
  //
  // The robust + portable path is to read the bytes ourselves and pass them
  // — wasm-bindgen 0.2.120 accepts a BufferSource directly. This avoids the
  // `fetch('file://...')` portability question entirely.
  const wasmPath = fileURLToPath(
    new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url),
  );
  const bytes = await readFile(wasmPath);
  // The InitFossilWasmOpts type narrows to URL/string/Request/Response in the
  // public API (the conservative, browser-shaped surface); BufferSource is a
  // wasm-bindgen-accepted input that we use here in the Node test harness via
  // a cast. The cast is test-internal and does not leak into the public API.
  await initFossilWasm({ wasmUrl: bytes as unknown as URL });
});

describe('@fossil-lang/wasm — tokenize', () => {
  it('returns a non-empty TokenRow[] for a prefix decl', () => {
    const rows: TokenRow[] = tokenize('prefix ex: <https://example.org/>\n');
    expect(rows.length).toBeGreaterThan(0);
    expect(rows[0]).toHaveProperty('kind');
    expect(rows[0]).toHaveProperty('start');
    expect(rows[0]).toHaveProperty('end');
    expect(rows[0]!.start).toBe(0);
  });

  it('returns empty array for empty source', () => {
    expect(tokenize('')).toEqual([]);
  });

  it('token ranges are monotonic non-overlapping (start ≤ end; next.start ≥ prev.end)', () => {
    const rows = tokenize(
      'prefix ex: <https://example.org/>\nuser := io.csv("u.csv")',
    );
    for (const r of rows) {
      expect(r.start).toBeLessThanOrEqual(r.end);
    }
    for (let i = 1; i < rows.length; i++) {
      expect(rows[i - 1]!.end).toBeLessThanOrEqual(rows[i]!.start);
    }
  });
});

describe('@fossil-lang/wasm — semanticLegend', () => {
  it('returns tokenTypes + tokenModifiers arrays', () => {
    const legend = semanticLegend();
    expect(Array.isArray(legend.tokenTypes)).toBe(true);
    expect(Array.isArray(legend.tokenModifiers)).toBe(true);
    // Phase-6 06-07 ships 10 tokenTypes (keyword, namespace, type, function,
    // property, string, number, operator, comment, variable). We assert
    // non-emptiness rather than the exact count so additive legend changes
    // don't break this test (mirrors 08-02's ADR-0030-friendly stance on the
    // TokenRow.kind contract).
    expect(legend.tokenTypes.length).toBeGreaterThan(0);
  });
});

describe('@fossil-lang/wasm — FossilPlayground class', () => {
  it('constructs without throwing', () => {
    const pg = new FossilPlayground();
    expect(pg).toBeInstanceOf(FossilPlayground);
    pg.free();
  });

  it('classification() returns the stdlib WASM manifest', () => {
    const pg = new FossilPlayground();
    try {
      const classes = pg.classification();
      expect(classes.length).toBeGreaterThan(0);
      expect(classes[0]).toHaveProperty('name');
      expect(classes[0]).toHaveProperty('wasm_class');
      expect(['pure_sql', 'native_udf_only']).toContain(classes[0]!.wasm_class);
    } finally {
      pg.free();
    }
  });

  it('openFile + check returns diagnostics array for an opened file', () => {
    const pg = new FossilPlayground();
    try {
      const handle = pg.openFile(
        'file:///test.fossil',
        'prefix ex: <https://example.org/>\n',
      );
      // FileHandle is opaque (wasm-bindgen class with private constructor) —
      // we cannot assert typeof handle === 'number'. We DO assert the handle
      // round-trips through subsequent operations (closeFile in the finally
      // block), which is the actual contract that matters to consumers.
      expect(handle).toBeDefined();
      const diags = pg.check();
      expect(Array.isArray(diags)).toBe(true);
      // A bare prefix decl on its own is incomplete (no mapping rules) — the
      // type checker emits at least one diagnostic. We assert structural
      // shape rather than exact count to stay robust against future
      // diagnostic-message tweaks (ADR-0030-friendly stance).
      if (diags.length > 0) {
        expect(diags[0]).toHaveProperty('uri');
        expect(diags[0]).toHaveProperty('range');
        expect(diags[0]).toHaveProperty('severity');
        expect(diags[0]).toHaveProperty('message');
      }
      pg.closeFile(handle);
    } finally {
      pg.free();
    }
  });
});
