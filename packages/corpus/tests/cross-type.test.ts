import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { afterAll, beforeAll, expect, it } from 'vitest';

import './boot.js';
import { open, type Corpus, type Frame } from '../src/corpus.js';
import type { QueryFn, QueryRow } from '../src/query.js';

/**
 * **A frame is one vertex type's `dense_id` space, and a cross-type relation is not in it.**
 *
 * `Author authored Paper` is tiled by `Author`'s `dense_id` in `by_source` — which is a correct
 * ADDRESS, and it is why `fossil_graph::plan::ReadPlan::window` hands that file to a window over
 * `Author`. What the rows inside it carry is a `dst_dense` in `Paper`'s numbering, and the two
 * numberings are both `BIGINT` and both start at zero. So a frame that joins `dst_dense` against
 * the drawn type's `dense_id` — or that asks whether it is one of the visible ids — matches, every
 * time, and draws a line between two vertices that are not related to each other at all. No error,
 * no warning, and the line is indistinguishable from a real one.
 *
 * **The fixture is the conformance corpus, relabelled.** Two copies of `Person` become `Author` and
 * `Paper`, and two copies of `Person knows Person` become `Author knows Author` and
 * `Author authored Paper`. That is not a contrivance of the collision: `dense_id` is dense from
 * zero within EVERY vertex type, so two types of similar size collide on almost every id they have.
 * What it buys is the control — the same bytes, framed twice — so the invariant can be stated
 * without counting anything:
 *
 * > adding a relation that leaves the drawn type changes nothing about the picture.
 *
 * Both shapes of the read are asserted, because they compose their edge URLs differently and
 * therefore fail differently: level 0 joins the adjacency's two id columns against the drawn tiles,
 * and a level read takes the coordinates off the edge row and asks only whether each end is a
 * visible `dense_id`.
 */

const require = createRequire(import.meta.url);
const CONFORMANCE = fileURLToPath(new URL('../../../apps/corpus/conformance/corpus', import.meta.url));

/** The manifest's own numbers, so a hard-coded stride shows up as a failure. */
const VERTEX_COUNT = 300;
const CHUNK_SIZE = 64;
const EDGE_COUNT = 596;

const scratch: string[] = [];
let query: QueryFn;
let person: Corpus;
let relabelled: Corpus;

/** The projection block every copy of `Person`'s payload carries, pyramid included. */
const VERTEX_PROJECTIONS = `projections:
- path: ''
  scale: 1
  file_type: parquet
  properties:
  - name: subject
    data_type: string
    is_primary: true
- path: l1/
  scale: 4
  file_type: parquet
  properties: []
- path: l2/
  scale: 16
  file_type: parquet
  properties: []
version: gar/v1
`;

/** The projection block every copy of `Person knows Person` carries. */
const EDGE_PROJECTIONS = `directed: true
projections:
- path: by_source/
  scale: 1
  aligned_by: src
  ordered: true
  file_type: parquet
  properties: []
- path: by_target/
  scale: 1
  aligned_by: dst
  ordered: true
  file_type: parquet
  properties: []
- path: l1/
  scale: 4
  aligned_by: src
  ordered: true
  file_type: parquet
  properties: []
- path: l2/
  scale: 16
  aligned_by: src
  ordered: true
  file_type: parquet
  properties: []
version: gar/v1
`;

/**
 * The conformance corpus's bytes under four names: two vertex types and two relations, one of
 * which leaves the type it starts at. Nothing is generated — a fourth writer of Parquet in this
 * repository is what `apps/corpus/integration/frame-levels.test.ts` says a test must not become.
 */
function relabel(): string {
  const root = mkdtempSync(join(tmpdir(), 'fossil-cross-type-'));
  scratch.push(root);
  mkdirSync(join(root, 'vertex'), { recursive: true });
  mkdirSync(join(root, 'edge'), { recursive: true });

  for (const type of ['Author', 'Paper']) {
    cpSync(join(CONFORMANCE, 'vertex/Person'), join(root, 'vertex', type), { recursive: true });
    writeFileSync(
      join(root, 'vertex', `${type}.vertex.yml`),
      `type: ${type}\nvertex_count: ${VERTEX_COUNT}\nchunk_size: ${CHUNK_SIZE}\n` +
        `prefix: vertex/${type}/\n${VERTEX_PROJECTIONS}`,
    );
  }

  for (const [src, label, dst] of [
    ['Author', 'knows', 'Author'],
    ['Author', 'authored', 'Paper'],
  ]) {
    const relation = `${src}_${label}_${dst}`;
    cpSync(join(CONFORMANCE, 'edge/Person_knows_Person'), join(root, 'edge', relation), {
      recursive: true,
    });
    rmSync(join(root, 'edge', relation, 'Person_knows_Person.edge.yml'));
    writeFileSync(
      join(root, 'edge', relation, `${relation}.edge.yml`),
      `src_type: ${src}\nedge_type: ${label}\ndst_type: ${dst}\nedge_count: ${EDGE_COUNT}\n` +
        `chunk_size: ${CHUNK_SIZE}\nsrc_chunk_size: ${CHUNK_SIZE}\ndst_chunk_size: ${CHUNK_SIZE}\n` +
        `prefix: edge/${relation}/\n${EDGE_PROJECTIONS}`,
    );
  }

  writeFileSync(
    join(root, 'graph.graph.yml'),
    "name: graph\nprefix: ''\ncontainer: files\nvertices:\n" +
      '- vertex/Author.vertex.yml\n- vertex/Paper.vertex.yml\nedges:\n' +
      '- edge/Author_knows_Author/Author_knows_Author.edge.yml\n' +
      '- edge/Author_authored_Paper/Author_authored_Paper.edge.yml\nversion: gar/v1\n',
  );
  return root;
}

afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

beforeAll(async () => {
  const dist = dirname(require.resolve('@duckdb/duckdb-wasm'));
  const db = await createDuckDB(
    {
      mvp: {
        mainModule: resolve(dist, './duckdb-mvp.wasm'),
        mainWorker: resolve(dist, './duckdb-node-mvp.worker.cjs'),
      },
      eh: {
        mainModule: resolve(dist, './duckdb-eh.wasm'),
        mainWorker: resolve(dist, './duckdb-node-eh.worker.cjs'),
      },
    },
    new ConsoleLogger(),
    NODE_RUNTIME,
  );
  await db.instantiate();
  const conn = db.connect();
  const spill = mkdtempSync(join(tmpdir(), 'fossil-cross-type-spill-'));
  scratch.push(spill);
  conn.query(`SET temp_directory = '${spill}'`);
  query = async (sql: string): Promise<QueryRow[]> =>
    conn.query(sql).toArray().map((row: { toJSON(): QueryRow }) => row.toJSON());
  person = await open(CONFORMANCE, { query });
  relabelled = await open(relabel(), { query });
}, 120_000);

/** The whole extent, nudged past the far edge so the box holds every vertex. */
const whole = async (corpus: Corpus, type: string) => {
  const extent = (await corpus.extent(type))!;
  return {
    x: extent.minX,
    y: extent.minY,
    w: extent.maxX - extent.minX + 1,
    h: extent.maxY - extent.minY + 1,
  };
};

const linksOf = (frame: Frame): number[] => [...frame.links];

it.each([0, 1])(
  'a relation that leaves the drawn type changes no line of level %i',
  async (level) => {
    const control = await person.frame({
      type: 'Person',
      level,
      pixels: 1024,
      ...(await whole(person, 'Person')),
    });
    const drawn = await relabelled.frame({
      type: 'Author',
      level,
      pixels: 1024,
      ...(await whole(relabelled, 'Author')),
    });

    expect(control.links.length).toBeGreaterThan(0);
    expect(drawn.marks).toBe(control.marks);
    expect(linksOf(drawn)).toEqual(linksOf(control));
  },
  120_000,
);

it('draws nothing at all for a type whose only relation leaves it', async () => {
  const drawn = await relabelled.frame({
    type: 'Paper',
    level: 0,
    pixels: 1024,
    ...(await whole(relabelled, 'Paper')),
  });
  expect(drawn.links.length).toBe(0);
  expect(drawn.marks).toBe(VERTEX_COUNT);
});
