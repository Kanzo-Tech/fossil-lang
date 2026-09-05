import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import './boot.js';
import { CorpusManifestError, resolveCorpus } from '../src/index.js';

/**
 * The addressing binding, on the manifests the rest of this package already uses.
 *
 * The *contract* lives in `apps/corpus/conformance/expected.json` and is executed by
 * `conformance.test.ts`, by `crates/fossil-graph/tests/conformance.rs` and by a second
 * implementation with no npm at all. What is here is the behaviour a table of addresses cannot
 * express: what the arithmetic refuses, and what a corpus that declares no adjacency at all
 * resolves to.
 *
 * **Nothing below calls a bare arithmetic function, because there are none left.** `tileOf`,
 * `tilesOf`, `shiftFor`, `tailRows` and `TILE_SHIFT` were exported from this package and were a
 * second implementation of `crates/fossil-graph/src/plan.rs`; every published vector below is now
 * executed the way a reader meets it — off a resolved corpus, whose answer comes out of the one
 * reader there is.
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
  tile_url: {
    vectors: Array<{
      container: 'files' | 'rowgroups';
      prefix: string;
      stem: 'chunk' | 'tile';
      tile: string;
      url: string;
    }>;
  };
  declared_count: {
    vectors: Array<{ count: string; chunk_size: number; tiles: string; tail_rows: string }>;
  };
};

/**
 * A corpus that says exactly what a vector row says, and nothing else.
 *
 * There is no bare composer to run a row against, because the package publishes none: a tile's URL
 * and a tile's number come off a `VertexAddress` or an `AdjacencyAddress`, and those come off a
 * manifest. So each row runs against a manifest built to say what the row says — which also asserts
 * the field the `tile_url` table is about, `container`, is read from `graph.graph.yml` rather than
 * guessed from a prefix.
 */
function corpusOf(options: {
  prefix: string;
  edgePrefix?: string;
  adjPrefix?: string;
  chunkSize: number;
  vertexCount: string;
  container: 'files' | 'rowgroups';
}) {
  const { prefix, edgePrefix = '', adjPrefix = '', chunkSize, vertexCount, container } = options;
  return resolveCorpus({
    manifestFiles: {
      'graph.graph.yml': [
        'name: graph',
        "prefix: ''",
        // Absent is `files`, so the row that says so is checked by omitting the field.
        ...(container === 'files' ? [] : [`container: ${container}`]),
        'vertices:',
        '- v.vertex.yml',
        ...(adjPrefix === '' ? [] : ['edges:', '- e.edge.yml']),
        'version: gar/v1',
        '',
      ].join('\n'),
      'v.vertex.yml':
        `type: T\nvertex_count: ${vertexCount}\nchunk_size: ${chunkSize}\n` +
        `prefix: ${prefix}/\nversion: gar/v1\n`,
      ...(adjPrefix === ''
        ? {}
        : {
            'e.edge.yml': [
              'src_type: T',
              'edge_type: knows',
              'dst_type: T',
              `chunk_size: ${chunkSize}`,
              `src_chunk_size: ${chunkSize}`,
              `dst_chunk_size: ${chunkSize}`,
              `prefix: ${edgePrefix}/`,
              'adj_lists:',
              '- ordered: true',
              '  aligned_by: src',
              `  prefix: ${adjPrefix}/`,
              '  file_type: parquet',
              'version: gar/v1',
              '',
            ].join('\n'),
          }),
    },
  });
}

describe('tileOf', () => {
  it('reproduces the published border vectors', () => {
    // The table's own shift is 12, so the corpus that answers it is one tiled at 4,096. 2³¹ is
    // where a port that took the shift as signed gives a negative tile; 2⁵³ is where one that went
    // through a `Number` stops being exact. Both are rows in the table.
    expect(vectors.tile_of.vectors.length).toBeGreaterThan(0);
    const person = corpusOf({
      prefix: 'v',
      chunkSize: 4096,
      vertexCount: '1',
      container: 'files',
    }).vertexType();
    for (const { dense_id, tile } of vectors.tile_of.vectors) {
      expect(person.tileOf(BigInt(dense_id)), dense_id).toBe(BigInt(tile));
    }
  });

  it('refuses a Number, because `>>` truncates to 32 bits before it shifts', () => {
    const person = corpusOf({
      prefix: 'v',
      chunkSize: 4096,
      vertexCount: '1',
      container: 'files',
    }).vertexType();
    expect(() => person.tileOf(4096 as unknown as bigint)).toThrow(TypeError);
    expect(() => person.tileOf(-1n)).toThrow(RangeError);
  });

  it('takes the corpus’s own shift, and not a default', () => {
    // The same id in two corpora tiled differently is two tiles, which is the whole reason the
    // shift is a manifest field and not a constant this package carries.
    const wide = corpusOf({ prefix: 'v', chunkSize: 4096, vertexCount: '1', container: 'files' });
    const narrow = corpusOf({ prefix: 'v', chunkSize: 64, vertexCount: '1', container: 'files' });
    expect(wide.vertexType().tileOf(64n)).toBe(0n);
    expect(narrow.vertexType().tileOf(64n)).toBe(1n);
    expect(wide.vertexType().shift).toBe(12);
    expect(narrow.vertexType().shift).toBe(6);
  });
});

describe('tileUrl', () => {
  const composed = (row: (typeof vectors.tile_url.vectors)[number]): string => {
    const prefix = row.prefix.replace(/\/+$/, '');
    // An adjacency's prefix is composed from two manifest fields — the edge type's and the
    // `adj_lists` entry's — so a row that publishes the whole path is split back into them here.
    const cut = prefix.lastIndexOf('/');
    const corpus = corpusOf({
      prefix,
      edgePrefix: prefix.slice(0, cut),
      adjPrefix: prefix.slice(cut + 1),
      chunkSize: 4096,
      vertexCount: '1',
      container: row.container,
    });
    expect(corpus.container).toBe(row.container);
    return row.stem === 'chunk'
      ? corpus.vertexType().tileUrl(BigInt(row.tile))
      : corpus.edges[0]!.adjacency('src')!.tileUrl(BigInt(row.tile));
  };

  it('reproduces the published border vectors', () => {
    expect(vectors.tile_url.vectors.length).toBeGreaterThan(0);
    for (const row of vectors.tile_url.vectors) {
      expect(composed(row), JSON.stringify(row)).toBe(row.url);
    }
  });

  it('still carries the rows a wrong port would pass', () => {
    // The same vacuity check the two tables here already carry. A table with no tile above 2^53
    // passes against a composer that interpolates a Number; a table with rows in one container
    // passes against a reader that never learned there are two; and a row-group half whose rows
    // all name different files passes against one that kept composing the tile into the name.
    const beyond = vectors.tile_url.vectors.filter(
      (v) => v.container === 'files' && !Number.isSafeInteger(Number(v.tile)),
    );
    expect(beyond.length).toBeGreaterThan(0);
    for (const v of beyond) expect(v.url).not.toContain(String(Number(v.tile)));

    expect(new Set(vectors.tile_url.vectors.map((v) => v.container)).size).toBe(2);
    const shared = vectors.tile_url.vectors.filter((v) => v.container === 'rowgroups');
    expect(new Set(shared.map((v) => v.url)).size).toBeLessThan(shared.length);
  });
});

describe('the declared count', () => {
  it('reproduces the published declared_count vectors', () => {
    // The borders that matter: 4,096 @ 4,096 is ONE tile (the off-by-one addresses a `chunk1` that
    // nothing wrote), 4,097 is two with a tail of one (the tile a truncated corpus loses), 300 @ 64
    // is the conformance corpus and catches a hard-coded stride, and 2⁵³+1 is where a `Number`
    // division comes out one tile short and the tail disappears from a reader that never asks.
    //
    // **`tail_rows` is not asserted here any more**, and that is the one column this package lost
    // when its arithmetic went: a tail is a free function of the reader and no resolved corpus
    // publishes it. `apps/corpus/guards` still executes that column, in plain Node.
    expect(vectors.declared_count.vectors.length).toBeGreaterThan(0);
    for (const v of vectors.declared_count.vectors) {
      const type = corpusOf({
        prefix: 'v',
        chunkSize: v.chunk_size,
        vertexCount: v.count,
        container: 'files',
      }).vertexType();
      expect(type.count, v.count).toBe(BigInt(v.count));
      expect(type.tiles, v.count).toBe(BigInt(v.tiles));
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
    expect(2 ** person.shift).toBe(declared);
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
    const addressed = corpus.tilesFor({ tiles: [0, 1], directions: ['src', 'dst'] });

    expect([...addressed.vertexUrls]).toEqual([
      'vertex/Person/chunk0.parquet',
      'vertex/Person/chunk1.parquet',
    ]);
    expect([...addressed.edgeUrls]).toEqual([]);
    expect(addressed.complete).toBe(false);
    expect(addressed.gaps).toEqual([
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
    expect(() => resolveCorpus({ manifestFiles: broken })).toThrow(CorpusManifestError);
    expect(() => resolveCorpus({ manifestFiles: broken })).toThrow('no shift addresses');
  });

  it('is synchronous once the module is up, and takes no fetch', () => {
    // **The claim moved and did not go.** It used to be that addressing needed nothing at all;
    // it needs an instantiated WASM module, which `./boot.js` gave this file. What it still does
    // not need is a request: a round trip between the camera moving and a URL being computable is
    // the `viewport` verb this format deleted, and `resolveCorpus` returns a corpus, not a promise.
    const corpus = resolveCorpus({ manifestFiles });
    expect(corpus).not.toBeInstanceOf(Promise);
    expect(corpus.tilesFor({ tiles: [7] }).vertexUrls).toEqual(['vertex/Person/chunk7.parquet']);
  });

  it('refuses a vertex type the manifest does not declare, and names the ones it does', () => {
    // The refusal has one author. A second copy of the type list on this side of the boundary is
    // what the whole change removed, so the sentence comes back from the reader.
    const corpus = resolveCorpus({ manifestFiles });
    expect(() => corpus.vertexType('Nobody')).toThrow(CorpusManifestError);
    expect(() => corpus.vertexType('Nobody')).toThrow('it names Person');
  });
});

/**
 * The `levels:` block — **the first thing a manifest says that is a sequence inside a mapping.**
 *
 * The shape below is not invented here: `fossil-sinks`' own
 * `the_levels_block_is_emitted_in_the_shape_the_line_scanners_read` asserts the emitter produces
 * exactly these bytes.
 *
 * **What is being defended is a silent failure and not a parse error.** A corpus WITH a pyramid
 * that resolves to a corpus without one throws nothing, diagnoses nothing, and opens a million rows
 * to draw fifteen thousand — the same failure `index:` had. Which is why the assertion below is on
 * the numbers surviving rather than on a URL.
 */
describe('levels — the written pyramid', () => {
  /**
   * A million vertices at 4,096 to a tile is the COMPLETE plan `VertexLevels::planned` writes:
   * 1, 2, 3, 4 — every level down to the one that fits a single tile, because in quarters the whole
   * pyramid costs a third of the type and there is nothing left for a window or a floor to bound.
   */
  const withLevels = (extra = 'levels:\n  prefix: l\n  levels:\n  - 1\n  - 2\n  - 3\n  - 4\n  chunk_size: 4096\n') => {
    const yaml = manifestFiles['vertex/Person.vertex.yml']!;
    const mutated = `${yaml.replace(/^vertex_count: \d+$/m, 'vertex_count: 1000000')}${extra}`;
    // The mutation has to have happened, for the reason the chunk_size test states at length.
    expect(mutated).toContain('vertex_count: 1000000');
    return { ...manifestFiles, 'vertex/Person.vertex.yml': mutated };
  };

  it('sees the level list, which is the whole of what it cannot derive', () => {
    const [person] = resolveCorpus({ manifestFiles: withLevels() }).types;
    expect(person!.levels?.levels).toEqual([1, 2, 3, 4]);
    expect(person!.levels?.chunkSize).toBe(4096);
    expect(person!.levels?.has(3)).toBe(true);
    expect(person!.levels?.has(5)).toBe(false);
  });

  it('addresses a level tile by the same shift, with 2k more bits falling off', () => {
    const levels = resolveCorpus({ manifestFiles: withLevels() }).types[0]!.levels!;
    // Level 3 keeps one id in 64 — `strideOf(3)`, which is 4³ and not 2³ — so a tile of 4,096 of
    // its rows spans 262,144 payload ids: the payload's own shift of 12 plus `strideBits(3)` of 6.
    expect(levels.tileOf(3, 0n)).toBe(0n);
    expect(levels.tileOf(3, 262_143n)).toBe(0n);
    expect(levels.tileOf(3, 262_144n)).toBe(1n);
    expect(levels.tileUrl(3, 1)).toBe('vertex/Person/l3/chunk1.parquet');
    // `ceil(1,000,000 / 64)` rows, which is four tiles of 4,096 — the pyramid's cost in tiles,
    // and the number the manifest's own `planned` was written against.
    expect(levels.rows(3)).toBe(15_625n);
    expect(levels.tiles(3)).toBe(4n);
    expect(levels.files(3)).toEqual([
      'vertex/Person/l3/chunk0.parquet',
      'vertex/Person/l3/chunk1.parquet',
      'vertex/Person/l3/chunk2.parquet',
      'vertex/Person/l3/chunk3.parquet',
    ]);
    // The coarsest the plan reaches fits one tile, which is where it stops: coarser buys nothing,
    // because one tile is already one range request and the whole level is the minimum read.
    expect(levels.rows(4)).toBe(3_907n);
    expect(levels.tiles(4)).toBe(1n);
  });

  it('refuses to address a level nobody wrote, and says what answers it instead', () => {
    const levels = resolveCorpus({ manifestFiles: withLevels() }).types[0]!.levels!;
    expect(() => levels.files(5)).toThrow('writes levels 1, 2, 3, 4 and not 5');
    expect(() => levels.files(5)).toThrow('the predicate over the payload');
  });

  it('refuses a block that declares a pyramid without its numbers', () => {
    // A prefix with no list is not a corpus without a pyramid: it is one whose policy the reader
    // cannot read, and the two must not resolve to the same thing — `index:` made that mistake.
    const files = withLevels('levels:\n  prefix: l\n  chunk_size: 4096\n');
    expect(() => resolveCorpus({ manifestFiles: files })).toThrow('no level list');
    expect(() => resolveCorpus({ manifestFiles: files })).toThrow('not arithmetic a reader can redo');
  });

  it('is null when the manifest declares none, which is a corpus and not a gap', () => {
    expect(resolveCorpus({ manifestFiles }).types[0]!.levels).toBeNull();
  });
});
