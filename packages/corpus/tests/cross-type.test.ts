import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { afterAll, beforeAll, expect, it } from 'vitest';

import './boot.js';
import { open, type Corpus, type EdgeAnswer } from '../src/index.js';
import { duckdb } from './engine.js';

/**
 * **A tile is one vertex type's `dense_id` space, and a cross-type relation is not in it.**
 *
 * `Author authored Paper` is tiled by `Author`'s `dense_id` in `by_source` — which is a correct
 * ADDRESS. What the rows inside it carry is a `dst_dense` in `Paper`'s numbering, and the two
 * numberings are both `BIGINT` and both start at zero. So a view that joins `dst_dense` against
 * the drawn type's `dense_id` matches, every time, and draws a line between two vertices that are
 * not related to each other at all. No error, no warning, and the line is indistinguishable from a
 * real one — which is why `edges` declines that relation unless the call names it.
 *
 * **The fixture is the conformance corpus, relabelled.** Two copies of `Person` become `Author` and
 * `Paper`, and two copies of `Person knows Person` become `Author knows Author` and
 * `Author authored Paper`. That is not a contrivance of the collision: `dense_id` is dense from
 * zero within EVERY vertex type, so two types of similar size collide on almost every id they have.
 * What it buys is the control — the same bytes, read twice — so the invariant can be stated
 * without counting anything:
 *
 * > adding a relation that leaves the drawn type changes nothing about the edges a tile answers.
 */

const CONFORMANCE = fileURLToPath(new URL('../conformance/corpus', import.meta.url));

/** The manifest's own numbers, so a hard-coded stride shows up as a failure. */
const VERTEX_COUNT = 300;
const CHUNK_SIZE = 64;
const EDGE_COUNT = 596;

const scratch: string[] = [];
let person: Corpus;
let relabelled: Corpus;

/** The projection block every copy of `Person`'s payload carries. */
const VERTEX_PROJECTIONS = `projections:
- path: ''
  scale: 1
  file_type: parquet
  properties:
  - name: subject
    data_type: string
    is_primary: true
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
version: gar/v1
`;

/**
 * The conformance corpus's bytes under four names: two vertex types and two relations, one of
 * which leaves the type it starts at. Nothing is generated: a test is not a fourth writer of Parquet
 * in this repository.
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
        `prefix: vertex/${type}/\ntile_manifest: tile-manifest.json\n${VERTEX_PROJECTIONS}`,
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
  const spill = mkdtempSync(join(tmpdir(), 'fossil-cross-type-spill-'));
  scratch.push(spill);
  const { engine } = await duckdb(spill);
  person = await open(CONFORMANCE, { engine });
  relabelled = await open(relabel(), { engine });
}, 120_000);

/** Every tile of one type's payload, read in one orientation. */
const everyTile = (corpus: Corpus, type: string, direction: 'src' | 'dst'): Promise<readonly EdgeAnswer[]> => {
  const matrices = corpus.tileMatrix(type).tileMatrices;
  const z = matrices.length - 1;
  const from = matrices[z]!.tiles.map((t) => ({ type, z, tile: t.tile }));
  return corpus.edges({ from, direction });
};

const linesOf = (answers: readonly EdgeAnswer[]): string[] =>
  answers.flatMap((a) =>
    a.batches.flatMap((b) => Array.from(b.src, (src, i) => `${b.edgeType} ${src} ${b.dst[i]}`)),
  );

it('a relation that leaves the drawn type changes no edge a tile answers', async () => {
  const control = await everyTile(person, 'Person', 'src');
  const drawn = await everyTile(relabelled, 'Author', 'src');

  expect(linesOf(control).length).toBe(EDGE_COUNT);
  expect(linesOf(drawn)).toEqual(linesOf(control));

  // And it is left out LOUDLY: an answer that quietly drops a relation is an answer about a graph
  // that does not exist.
  expect(control.every((a) => a.declined.length === 0)).toBe(true);
  for (const answer of drawn) {
    expect(answer.declined).toEqual([{ edgeType: 'authored', direction: 'src', reason: 'other-space' }]);
  }
}, 120_000);

it('answers no edge at all for a type whose only relation leaves it', async () => {
  const drawn = await everyTile(relabelled, 'Paper', 'dst');
  expect(drawn).toHaveLength(Math.ceil(VERTEX_COUNT / CHUNK_SIZE));
  expect(linesOf(drawn)).toEqual([]);
  for (const answer of drawn) {
    expect(answer.declined).toEqual([{ edgeType: 'authored', direction: 'dst', reason: 'other-space' }]);
  }
});

it('edges from a tile read the relations cut on its type, and decline the one that leaves it', async () => {
  const author = { type: 'Author', z: 0, tile: 1 };
  const out = (await relabelled.edges({ from: [author], direction: 'src' }))[0]!;
  expect(out.batches.map((b) => [b.edgeType, b.srcType, b.dstType])).toEqual([['knows', 'Author', 'Author']]);
  expect(out.declined).toEqual([{ edgeType: 'authored', direction: 'src', reason: 'other-space' }]);

  // Named, the relation that leaves the type is read: its far ends are `Paper` ids, and the batch
  // says so rather than letting them pass for `Author`s.
  const named = (await relabelled.edges({ from: [author], direction: 'src', relation: 'authored' }))[0]!;
  expect(named.batches.map((b) => [b.edgeType, b.srcType, b.dstType])).toEqual([['authored', 'Author', 'Paper']]);
  expect(named.batches[0]!.src.length).toBe(out.batches[0]!.src.length);
  expect(named.declined).toEqual([]);

  // A `Paper` is the target of `authored` and the source of nothing.
  const paper = { type: 'Paper', z: 0, tile: 1 };
  expect((await relabelled.edges({ from: [paper], direction: 'src' }))[0]).toEqual({ batches: [], declined: [] });
  expect((await relabelled.edges({ from: [paper], direction: 'dst' }))[0]!.declined).toEqual([
    { edgeType: 'authored', direction: 'dst', reason: 'other-space' },
  ]);
  const into = (await relabelled.edges({ from: [paper], direction: 'dst', relation: 'authored' }))[0]!;
  expect(into.batches[0]!.dst.every((d) => d >= 64n && d < 128n)).toBe(true);
});
