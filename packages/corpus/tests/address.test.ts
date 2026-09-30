import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import './boot.js';
import { CorpusManifestError, addressManifests } from '../src/address.js';
import { addressingOf } from './addressing-of.js';

/**
 * The addressing binding, on the manifests the rest of this package already uses.
 *
 * The *contract* lives in `packages/corpus/conformance/expected.json` and is executed by
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
 * `packages/corpus/guards/vectors.json` is the deliverable — the file a third party copies — and every
 * other implementation reads it instead of transcribing it: the guard `published-vectors`, and a
 * Rust test on the writer's side. A table transcribed here would be a fourth copy that drifts
 * silently, which is the pathology the whole file exists to argue against.
 */
const vectors = JSON.parse(
  await readFile(
    fileURLToPath(new URL('../guards/vectors.json', import.meta.url)),
    'utf8',
  ),
) as {
  tile_of: { vectors: Array<{ dense_id: string; tile: string }> };
  tile_url: {
    vectors: Array<{
      container: 'files' | 'rowgroups';
      prefix: string;
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
 * and a tile's number come off a `VertexAddress` or a `ProjectionAddress`, and those come off a
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
  // The binding itself and not an open: a `files` corpus declaring 2⁵³ rows is a row of the table,
  // and an open enumerates every payload file of every type.
  return addressManifests(
    {
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
        `prefix: ${prefix}/\nprojections:\n- path: ''\n  scale: 1\n  file_type: parquet\n` +
        'version: gar/v1\n',
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
              'projections:',
              `- path: ${adjPrefix}/`,
              '  scale: 1',
              '  aligned_by: src',
              '  ordered: true',
              '  file_type: parquet',
              'version: gar/v1',
              '',
            ].join('\n'),
          }),
    },
    '',
  );
}

describe('tileOf', () => {
  it('reproduces the published border vectors', async () => {
    // The table's own shift is 12, so the corpus that answers it is one tiled at 4,096. 2³¹ is
    // where a port that took the shift as signed gives a negative tile; 2⁵³ is where one that went
    // through a `Number` stops being exact. Both are rows in the table.
    expect(vectors.tile_of.vectors.length).toBeGreaterThan(0);
    const person = (corpusOf({
      prefix: 'v',
      chunkSize: 4096,
      vertexCount: '1',
      container: 'files',
    })).vertexType();
    for (const { dense_id, tile } of vectors.tile_of.vectors) {
      expect(person.tilesOf([BigInt(dense_id)])[0], dense_id).toBe(BigInt(tile));
    }
  });

  it('refuses a Number, because `>>` truncates to 32 bits before it shifts', async () => {
    const person = (corpusOf({
      prefix: 'v',
      chunkSize: 4096,
      vertexCount: '1',
      container: 'files',
    })).vertexType();
    expect(() => person.tilesOf([4096 as unknown as bigint])[0]).toThrow(TypeError);
    expect(() => person.tilesOf([-1n])[0]).toThrow(RangeError);
  });

  it('takes the corpus’s own shift, and not a default', async () => {
    // The same id in two corpora tiled differently is two tiles, which is the whole reason the
    // shift is a manifest field and not a constant this package carries.
    const wide = corpusOf({ prefix: 'v', chunkSize: 4096, vertexCount: '1', container: 'files' });
    const narrow = corpusOf({ prefix: 'v', chunkSize: 64, vertexCount: '1', container: 'files' });
    expect(wide.vertexType().tilesOf([64n])[0]).toBe(0n);
    expect(narrow.vertexType().tilesOf([64n])[0]).toBe(1n);
    expect(wide.vertexType().chunkSize).toBe(4096);
    expect(narrow.vertexType().chunkSize).toBe(64);
  });
});

describe('tileUrl', () => {
  /**
   * The row composed **twice** — off the vertex payload and off an adjacency at the same prefix.
   *
   * The table dispatched on a `stem` column while an adjacency spelled its files `tile{k}`; there
   * are no two stems left, so both members compose the identical URL and the row is executed
   * against both. Which is what its own `why` says: an adjacency is the projection at `scale: 1`
   * and not a different kind of artefact.
   */
  const composed = async (row: (typeof vectors.tile_url.vectors)[number]): Promise<string[]> => {
    const prefix = row.prefix.replace(/\/+$/, '');
    // An adjacency's prefix is composed from two manifest fields — the edge type's and the
    // projection's own `path` — so a row that publishes the whole path is split back into them.
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
    return [
      corpus.vertexType().tileUrl(BigInt(row.tile)),
      corpus.edges[0]!.adjacency('src')!.tileUrl(BigInt(row.tile)),
    ];
  };

  it('reproduces the published border vectors', async () => {
    expect(vectors.tile_url.vectors.length).toBeGreaterThan(0);
    for (const row of vectors.tile_url.vectors) {
      expect(await composed(row), JSON.stringify(row)).toEqual([row.url, row.url]);
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
  it('reproduces the published declared_count vectors', async () => {
    // The borders that matter: 4,096 @ 4,096 is ONE tile (the off-by-one addresses a `chunk1` that
    // nothing wrote), 4,097 is two with a tail of one (the tile a truncated corpus loses), 300 @ 64
    // is the conformance corpus and catches a hard-coded stride, and 2⁵³+1 is where a `Number`
    // division comes out one tile short and the tail disappears from a reader that never asks.
    //
    // **`tail_rows` is not asserted here any more**, and that is the one column this package lost
    // when its arithmetic went: a tail is a free function of the reader and no resolved corpus
    // publishes it. `packages/corpus/guards` still executes that column, in plain Node.
    expect(vectors.declared_count.vectors.length).toBeGreaterThan(0);
    for (const v of vectors.declared_count.vectors) {
      const type = (corpusOf({
        prefix: 'v',
        chunkSize: v.chunk_size,
        vertexCount: v.count,
        container: 'files',
      })).vertexType();
      expect(type.count, v.count).toBe(BigInt(v.count));
      expect(type.tiles, v.count).toBe(BigInt(v.tiles));
    }
  });

  it('still carries a row a Number implementation would fail', () => {
    // The table proves nothing about the width if every row fits in 53 bits. This is the check that
    // the separating row has not been dropped, which is how a published table quietly stops being
    // evidence — the same guard the quantisation carries for its binary32 row.
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

describe('the addressing — the arithmetic over the manifests', () => {
  it('addresses vertex tiles under the declared prefix', async () => {
    const corpus = await addressingOf('/bench/1000000', manifestFiles);
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
    expect(person.chunkSize).toBe(declared);
    // The fixture declares `container: rowgroups`, which is what fossil writes: every tile of the
    // set names one file and the footer's box on `dense_id` says which row groups are the tile.
    // The ordinal-in-the-name half of the claim is `packages/corpus/integration/containers.test.ts`'s
    // and the vector table's.
    expect(person.tileUrl(0)).toBe('/bench/1000000/vertex/Person/tiles.parquet');
    // The boundary is what the shift is FOR, so it is asserted at the boundary
    // wherever the fixture puts it.
    expect(person.tilesOf([BigInt(declared) - 1n])[0]).toBe(0n);
    expect(person.tilesOf([BigInt(declared)])[0]).toBe(1n);
  });

  it('enumerates every addressable file once, payload first, for a host to grant whole', async () => {
    const addressing = await addressingOf('', manifestFiles);
    const files = addressing.files();
    expect(new Set(files).size).toBe(files.length);
    const want: string[] = [];
    for (const type of addressing.types) {
      want.push(...type.files(), ...(type.index?.files() ?? []));
    }
    for (const edge of addressing.edges) {
      for (const d of edge.directions) want.push(...edge.adjacencyFiles(d));
    }
    expect(files).toEqual([...new Set(want)]);
    expect(files).toContain(addressing.types[0]!.tileUrl(0));
  });

  it('addresses relative to the dataset root when the base is empty', async () => {
    const corpus = await addressingOf('', manifestFiles);
    expect(corpus.vertexType().tileUrl(9)).toBe('vertex/Person/tiles.parquet');
  });

  it('publishes no orientation for an edge type that declares none', async () => {
    // This fixture's `projections` is empty — a manifest that says the relation exists and does
    // not say where any of it is. Composing `by_source/chunk{k}.parquet` from the convention is
    // exactly the 404 this module exists to make impossible.
    const corpus = await addressingOf('', manifestFiles);
    const knows = corpus.edges[0]!;

    expect(knows.edgeType).toBe('knows');
    expect(knows.directions).toEqual([]);
    expect(knows.adjacency('src')).toBeNull();
    expect(knows.adjacency('dst')).toBeNull();
  });

  it('names the file when a manifest it was promised is not there', async () => {
    const { 'vertex/Person.vertex.yml': _dropped, ...without } = manifestFiles;
    await expect(addressingOf('', without)).rejects.toThrow(CorpusManifestError);
    await expect(addressingOf('', without)).rejects.toThrow(
      'vertex/Person.vertex.yml',
    );
  });

  it('refuses a chunk_size no shift addresses', async () => {
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
    await expect(addressingOf('', broken)).rejects.toThrow(CorpusManifestError);
    await expect(addressingOf('', broken)).rejects.toThrow('no shift addresses');
  });

  it('computes a URL without a request', async () => {
    // A round trip between the camera moving and a URL being computable is the `viewport` verb
    // this format deleted. The addressing is resolved once, at open, and every URL after that is
    // arithmetic over it.
    const corpus = await addressingOf('', manifestFiles);
    expect(corpus.vertexType().tileUrl(7)).toBe('vertex/Person/tiles.parquet');
  });

  it('refuses a vertex type the manifest does not declare, and names the ones it does', async () => {
    // The refusal has one author. A second copy of the type list on this side of the boundary is
    // what the whole change removed, so the sentence comes back from the reader.
    const corpus = await addressingOf('', manifestFiles);
    expect(() => corpus.vertexType('Nobody')).toThrow(CorpusManifestError);
    expect(() => corpus.vertexType('Nobody')).toThrow('it names Person');
  });
});

/**
 * The `projections:` list, as the reader refuses it. The payload is the entry at `scale: 1`, and a
 * scale is a product the reader shifts by — so one that is no power of two is refused, and a list
 * with no `scale: 1` has no payload to address.
 */
describe('projections — what the reader refuses', () => {
  const withProjection = (entry: string) => {
    const yaml = manifestFiles['vertex/Person.vertex.yml']!;
    const mutated = yaml.replace(/^version: gar\/v1$/m, `${entry}version: gar/v1`);
    expect(mutated).not.toBe(yaml);
    return { ...manifestFiles, 'vertex/Person.vertex.yml': mutated };
  };

  it('refuses a scale no shift addresses, because a projection is never a division', async () => {
    const files = withProjection('- path: coarse/\n  scale: 3\n  file_type: parquet\n');
    await expect(addressingOf('', files)).rejects.toThrow(CorpusManifestError);
    await expect(addressingOf('', files)).rejects.toThrow('scale 3, which no shift addresses');
  });

  it('refuses a type with no payload, which the count and the cut describe and nothing addresses', async () => {
    const yaml = manifestFiles['vertex/Person.vertex.yml']!;
    const mutated = yaml.replace(/^- path: ''\n  scale: 1\n/m, `- path: coarse/\n  scale: 4\n`);
    expect(mutated).not.toBe(yaml);
    const files = { ...manifestFiles, 'vertex/Person.vertex.yml': mutated };
    await expect(addressingOf('', files)).rejects.toThrow('no projection at scale 1');
    await expect(addressingOf('', files)).rejects.toThrow('no payload to address');
  });
});
