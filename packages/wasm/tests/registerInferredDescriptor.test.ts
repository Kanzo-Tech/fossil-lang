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
        source_name: 'users',
        columns: [
          { name: 'id', primitive: 'Integer' },
          { name: 'name', primitive: 'String' },
        ],
        content_hash: '',
      };
      expect(() => pg.registerInferredDescriptor(desc)).not.toThrow();
    } finally {
      pg.free();
    }
  });

  it('re-registering the same source_name does not throw', () => {
    const pg = new FossilPlayground();
    try {
      const first: InferredDescriptorJson = {
        source_name: 'users',
        columns: [{ name: 'id', primitive: 'Integer' }],
        content_hash: 'h1',
      };
      const second: InferredDescriptorJson = {
        source_name: 'users',
        columns: [
          { name: 'id', primitive: 'Integer' },
          { name: 'email', primitive: 'String' },
        ],
        content_hash: 'h2',
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
      const malformed = { source_name: 'users' } as unknown as InferredDescriptorJson;
      expect(() => pg.registerInferredDescriptor(malformed)).toThrow();
    } finally {
      pg.free();
    }
  });

  it('registers multiple distinct sources independently', () => {
    const pg = new FossilPlayground();
    try {
      pg.registerInferredDescriptor({
        source_name: 'users',
        columns: [{ name: 'id', primitive: 'Integer' }],
        content_hash: '',
      });
      pg.registerInferredDescriptor({
        source_name: 'products',
        columns: [{ name: 'sku', primitive: 'String' }],
        content_hash: '',
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
        source_name: 'empty',
        columns: [],
        content_hash: '',
      };
      expect(() => pg.registerInferredDescriptor(desc)).not.toThrow();
    } finally {
      pg.free();
    }
  });
});
