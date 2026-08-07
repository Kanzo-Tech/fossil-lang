/**
 * Vitest coverage for FossilPlayground.registerInferredDescriptor (plan 13-03,
 * ADR-0037). Exercises the wasm-bindgen build output directly — these tests
 * are integration-flavoured (real WASM load) but isolated to the
 * registration API (no compile call yet — that comes in plan 13-04b's
 * playground orchestration).
 *
 * The Rust side is covered by `crates/fossil-wasm/tests/register_inferred_descriptor.rs`
 * (6 tests against the pure-Rust `*_native` helper). This suite covers the
 * additional wasm-bindgen serialisation boundary the cargo-tests can't reach.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import {
  initFossilWasm,
  FossilPlayground,
  type InferredDescriptorJson,
} from '../src/index.js';

beforeAll(async () => {
  // Mirrors the tokenize.test.ts bootstrap — see that file for the
  // file://-vs-BufferSource rationale.
  const wasmPath = fileURLToPath(
    new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url),
  );
  const bytes = await readFile(wasmPath);
  await initFossilWasm({ wasmUrl: bytes as unknown as URL });
});

describe('FossilPlayground.registerInferredDescriptor', () => {
  it('registers a valid descriptor without throwing', () => {
    const pg = new FossilPlayground();
    try {
      const desc: InferredDescriptorJson = {
        uri: 'users.csv',
        columns: [
          { name: 'id', primitive: 'integer' },
          { name: 'name', primitive: 'string' },
        ],
        freshness_token: '',
      };
      expect(() => pg.registerInferredDescriptor(desc)).not.toThrow();
    } finally {
      pg.free();
    }
  });

  it('re-registering the same uri does not throw', () => {
    const pg = new FossilPlayground();
    try {
      const first: InferredDescriptorJson = {
        uri: 'users.csv',
        columns: [{ name: 'id', primitive: 'integer' }],
        freshness_token: 'h1',
      };
      const second: InferredDescriptorJson = {
        uri: 'users.csv',
        columns: [
          { name: 'id', primitive: 'integer' },
          { name: 'email', primitive: 'string' },
        ],
        freshness_token: 'h2',
      };
      pg.registerInferredDescriptor(first);
      expect(() => pg.registerInferredDescriptor(second)).not.toThrow();
    } finally {
      pg.free();
    }
  });

  it('throws on missing required fields', () => {
    const pg = new FossilPlayground();
    try {
      // Bypass TS to exercise runtime Rust-side validation.
      const malformed = { uri: 'users.csv' } as unknown as InferredDescriptorJson;
      expect(() => pg.registerInferredDescriptor(malformed)).toThrow();
    } finally {
      pg.free();
    }
  });

  it('registers multiple distinct uris independently', () => {
    const pg = new FossilPlayground();
    try {
      pg.registerInferredDescriptor({
        uri: 'users.csv',
        columns: [{ name: 'id', primitive: 'integer' }],
        freshness_token: '',
      });
      pg.registerInferredDescriptor({
        uri: 'products.csv',
        columns: [{ name: 'sku', primitive: 'string' }],
        freshness_token: '',
      });
      // Both registrations should succeed; no cross-contamination. The
      // observable read path lives on the Rust side (covered by the
      // cargo-test suite); here we just confirm both calls return without
      // throwing.
      expect(true).toBe(true);
    } finally {
      pg.free();
    }
  });

  it('accepts empty columns array', () => {
    const pg = new FossilPlayground();
    try {
      const desc: InferredDescriptorJson = {
        uri: 'empty.csv',
        columns: [],
        freshness_token: '',
      };
      expect(() => pg.registerInferredDescriptor(desc)).not.toThrow();
    } finally {
      pg.free();
    }
  });
});
