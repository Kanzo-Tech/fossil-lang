import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import {
  CorpusManifestError,
  resolveCorpus,
  shiftFor,
  tailRows,
  tileOf,
  tilesOf,
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

/**
 * The published vectors, **executed rather than restated**.
 *
 * `apps/corpus/guards/vectors.json` is the deliverable — the file a third party copies — and every
 * other implementation reads it instead of transcribing it: the guard `published-vectors`, and a
 * Rust test on the writer's side. A table transcribed here would be a fourth copy that drifts
 * silently, which is the pathology the whole file exists to argue against.
 */
const vectors = JSON.parse(
  await readFile(
    fileURLToPath(new URL('../../../apps/corpus/guards/vectors.json', import.meta.url)),
    'utf8',
  ),
) as {
  tile_of: { vectors: Array<{ dense_id: string; tile: string }> };
  declared_count: {
    vectors: Array<{ count: string; chunk_size: number; tiles: string; tail_rows: string }>;
  };
};

describe('tileOf', () => {
  it('reproduces the published border vectors', () => {
    // 2³¹ is where a port that took the shift as signed gives a negative tile; 2⁵³ is where one
    // that went through a `Number` stops being exact. Both are rows in the table below.
    expect(vectors.tile_of.vectors.length).toBeGreaterThan(0);
    for (const { dense_id, tile } of vectors.tile_of.vectors) {
      expect(tileOf(BigInt(dense_id))).toBe(BigInt(tile));
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

describe('tilesOf', () => {
  it('reproduces the published declared_count vectors', () => {
    // The borders that matter: 4,096 @ 4,096 is ONE tile (the off-by-one addresses a `chunk1` that
    // nothing wrote), 4,097 is two with a tail of one (the tile a truncated corpus loses), 300 @ 64
    // is the conformance corpus and catches a hard-coded stride, and 2⁵³+1 is where a `Number`
    // division comes out one tile short and the tail disappears from a reader that never asks.
    expect(vectors.declared_count.vectors.length).toBeGreaterThan(0);
    for (const v of vectors.declared_count.vectors) {
      const chunk = BigInt(v.chunk_size);
      expect(tilesOf(BigInt(v.count), chunk)).toBe(BigInt(v.tiles));
      expect(tailRows(BigInt(v.count), chunk)).toBe(BigInt(v.tail_rows));
    }
  });

  it('still carries a row a Number implementation would fail', () => {
    // The table proves nothing about the width if every row fits in 53 bits. This is the check that
    // the separating row has not been dropped, which is how a published table quietly stops being
    // evidence — the same guard the Morton half carries for its binary32 row.
    const beyond = vectors.declared_count.vectors.filter(
      (v) => !Number.isSafeInteger(Number(v.count)),
    );
    expect(beyond.length).toBeGreaterThan(0);
    for (const v of beyond) {
      const naive = BigInt(Math.ceil(Number(v.count) / v.chunk_size));
      expect(naive).not.toBe(BigInt(v.tiles));
    }
  });

  it('refuses a Number count, and a chunk size no shift addresses', () => {
    expect(() => tilesOf(300 as unknown as bigint, 64n)).toThrow(TypeError);
    expect(() => tilesOf(-1n, 64n)).toThrow(RangeError);
    expect(tilesOf(300n, 122_880n)).toBeNull();
    expect(tailRows(300n, 122_880n)).toBeNull();
  });
});

describe('resolveCorpus', () => {
  it('addresses vertex tiles under the declared prefix', () => {
    const corpus = resolveCorpus({ manifestFiles, base: '/bench/1000000' });
    const person = corpus.vertexType();

    // Derived from the fixture, not transcribed from it. `chunkSize` was
    // asserted as the literal 1024 and stayed green for months over a fixture
    // nothing regenerated — `dump_fixture` writes 4,096 and has since
    // `c416e07`. A constant copied out of a generated file is a second
    // spelling of that file, and it is the copy that goes stale.
    const declared = Number(
      /^chunk_size: (\d+)$/m.exec(manifestFiles['vertex/Person.vertex.yml']!)![1]!,
    );
    expect(person.type).toBe('Person');
    expect(person.chunkSize).toBe(declared);
    expect(2 ** Number(person.shift)).toBe(declared);
    expect(person.tileUrl(0)).toBe('/bench/1000000/vertex/Person/chunk0.parquet');
    // The boundary is what the shift is FOR, so it is asserted at the boundary
    // wherever the fixture puts it.
    expect(person.tileOf(BigInt(declared) - 1n)).toBe(0n);
    expect(person.tileOf(BigInt(declared))).toBe(1n);
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
    // 122,880 is `DuckDB`'s default row-group size and not a power of two, so
    // no shift names a tile.
    const yaml = manifestFiles['vertex/Person.vertex.yml']!;
    const mutated = yaml.replace(/^chunk_size: \d+$/m, 'chunk_size: 122880');
    // **The mutation has to have happened.** This replaced the literal string
    // `chunk_size: 1024`, and when the fixture stopped saying 1024 the replace
    // became a no-op — leaving the test asserting that an UNMODIFIED manifest
    // throws, which is a different claim and a false one. A mutation test whose
    // mutation can silently miss is a test of the thing it meant to break.
    expect(mutated).not.toBe(yaml);

    const broken = { ...manifestFiles, 'vertex/Person.vertex.yml': mutated };
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
