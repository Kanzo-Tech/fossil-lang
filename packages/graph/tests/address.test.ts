import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import {
  CorpusManifestError,
  resolveCorpus,
  shiftFor,
  tileOf,
  TILE_SHIFT,
} from '../src/index.js';

/**
 * The addressing module, on the manifests the rest of this package already uses.
 *
 * The *contract* lives in `apps/corpus/conformance/expected.json` and is executed by
 * `conformance.test.ts` and by a second implementation with no npm at all. What is here is the
 * behaviour a table of addresses cannot express: what the arithmetic refuses, and what a corpus
 * that declares no adjacency at all resolves to.
 */

const manifestFiles: Record<string, string> = JSON.parse(
  await readFile(fileURLToPath(new URL('./fixtures/manifest.json', import.meta.url)), 'utf8'),
) as Record<string, string>;

describe('tileOf', () => {
  it('reproduces the published border vectors', () => {
    // The same table `apps/corpus/guards/vectors.json` publishes and `fossil-sinks` asserts in
    // Rust. 2³¹ is where a port that took the shift as signed gives a negative tile; 2⁵³ is where
    // one that went through a `Number` stops being exact.
    for (const [denseId, tile] of [
      [0n, 0n],
      [4_095n, 0n],
      [4_096n, 1n],
      [8_191n, 1n],
      [2_147_483_647n, 524_287n],
      [2_147_483_648n, 524_288n],
      [9_007_199_254_740_992n, 2_199_023_255_552n],
    ] as const) {
      expect(tileOf(denseId)).toBe(tile);
    }
  });

  it('refuses a Number, because `>>` truncates to 32 bits before it shifts', () => {
    expect(() => tileOf(4096 as unknown as bigint)).toThrow(TypeError);
    expect(() => tileOf(-1n)).toThrow(RangeError);
  });

  it('takes the corpus’s own shift, not the default', () => {
    expect(tileOf(64n, 6n)).toBe(1n);
    expect(tileOf(64n)).toBe(0n);
    expect(shiftFor(4096)).toBe(TILE_SHIFT);
    // A tile size that is not a power of two forces a division where a shift does — 122,880 is
    // DuckDB's default row-group size and the corpus's tile size before it was measured.
    expect(shiftFor(122_880)).toBeNull();
    expect(shiftFor(0)).toBeNull();
  });
});

describe('resolveCorpus', () => {
  it('addresses vertex tiles under the declared prefix', () => {
    const corpus = resolveCorpus({ manifestFiles, base: '/bench/1000000' });
    const person = corpus.vertexType();

    expect(person.type).toBe('Person');
    expect(person.chunkSize).toBe(1024);
    expect(person.shift).toBe(10n);
    expect(person.tileUrl(0)).toBe('/bench/1000000/vertex/Person/chunk0.parquet');
    expect(person.tileOf(1023n)).toBe(0n);
    expect(person.tileOf(1024n)).toBe(1n);
  });

  it('addresses relative to the dataset root when no base is given', () => {
    const corpus = resolveCorpus({ manifestFiles });
    expect(corpus.vertexType().tileUrl(9)).toBe('vertex/Person/chunk9.parquet');
  });

  it('publishes no orientation for an edge type that declares none', () => {
    // This fixture's `adj_lists` is empty — a manifest that says the relation exists and does not
    // say where any of it is. Composing `by_source/tile{k}.parquet` from the convention is exactly
    // the 404 this module exists to make impossible.
    const corpus = resolveCorpus({ manifestFiles });
    const knows = corpus.edges[0]!;

    expect(knows.edgeType).toBe('knows');
    expect(knows.directions).toEqual([]);
    expect(knows.adjacency('src')).toBeNull();
    expect(knows.adjacency('dst')).toBeNull();
  });

  it('reports an unaddressable orientation as a gap rather than drawing nothing', () => {
    const corpus = resolveCorpus({ manifestFiles });
    const window = corpus.window({ tiles: [0, 1], directions: ['src', 'dst'] });

    expect([...window.vertexUrls]).toEqual([
      'vertex/Person/chunk0.parquet',
      'vertex/Person/chunk1.parquet',
    ]);
    expect([...window.edgeUrls]).toEqual([]);
    expect(window.complete).toBe(false);
    expect(window.gaps).toEqual([
      { edgeType: 'knows', direction: 'src', reason: 'not-declared' },
      { edgeType: 'knows', direction: 'dst', reason: 'not-declared' },
    ]);
  });

  it('names the file when a manifest it was promised is not there', () => {
    const { 'vertex/Person.vertex.yml': _dropped, ...without } = manifestFiles;
    expect(() => resolveCorpus({ manifestFiles: without })).toThrow(CorpusManifestError);
    expect(() => resolveCorpus({ manifestFiles: without })).toThrow('vertex/Person.vertex.yml');
  });

  it('refuses a chunk_size no shift addresses', () => {
    const broken = {
      ...manifestFiles,
      'vertex/Person.vertex.yml': manifestFiles['vertex/Person.vertex.yml']!.replace(
        'chunk_size: 1024',
        'chunk_size: 122880',
      ),
    };
    expect(() => resolveCorpus({ manifestFiles: broken })).toThrow('no shift addresses');
  });

  it('is synchronous, and takes no fetch', () => {
    // The claim, as a type: a request between the camera moving and a URL being computable is the
    // `viewport` verb this format deleted. `resolveCorpus` returns a corpus, not a promise.
    const corpus = resolveCorpus({ manifestFiles });
    expect(corpus).not.toBeInstanceOf(Promise);
    expect(corpus.window({ tiles: [7] }).vertexUrls).toEqual(['vertex/Person/chunk7.parquet']);
  });
});
