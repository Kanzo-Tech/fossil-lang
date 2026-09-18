import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import './boot.js';
import type { Direction, CorpusAddressing } from '../src/address.js';
import { openCorpus } from '../src/corpus.js';

/**
 * The conformance corpus, executed against the published module.
 *
 * `apps/corpus/conformance/expected.json` is a table of addresses — what a reader must compose from
 * a manifest and what it must refuse to compose. This file runs it through the wasm32 build that
 * actually ships, which is a different claim from `crates/fossil-graph/tests/conformance.rs`
 * running the same reader natively: `usize` is 64 bits there and 32 here, and 2^53 is where a port
 * that went through a double stops being exact. `apps/corpus/conformance/verify.mjs` is the leg
 * that is a separate implementation, in plain Node with no npm at all, because the format's claim
 * is that a reader who has never heard of this package can open the same corpus. None of the three
 * wrote the table.
 *
 * The `corpus` case has bytes: 300 vertices in five tiles of 64, both orientations tiled, passing
 * all fifteen guards. Every address it lists is checked to name a file that is on disk, which is
 * the failure this whole seam exists to prevent — a URL that composes cleanly and 404s in a
 * browser with no type error and no failing test.
 */

const CONFORMANCE = fileURLToPath(new URL('../../../apps/corpus/conformance/', import.meta.url));

interface Adjacency {
  direction: Direction;
  prefix: string;
  column: string;
  chunk_size: number;
  shift: number;
}

interface Case {
  name: string;
  root: string;
  on_disk: boolean;
  container?: 'files' | 'rowgroups';
  types?: Array<{ type: string; prefix: string; chunk_size: number; shift: number }>;
  edges?: Array<{
    edge_type: string;
    src_type: string;
    dst_type: string;
    prefix: string;
    directions: Direction[];
    adjacencies: Adjacency[];
  }>;
  tile_of?: Array<{ type: string; dense_id: string; tile: string }>;
  addresses?: Array<{
    kind: 'vertex' | 'edge';
    type?: string;
    edge_type?: string;
    direction?: Direction;
    tile: number;
    path: string;
  }>;
  refused?: Array<{ edge_type: string; direction: Direction }>;
  windows?: Array<{
    type?: string;
    tiles: number[];
    directions: Direction[];
    vertex_urls: string[];
    edge_urls: string[];
    complete: boolean;
    gaps: Array<{ edge_type: string; direction: Direction; reason: string }>;
  }>;
  throws?: Array<{ vertex_type: string; message: string }>;
  resolve_throws?: string;
  /**
   * The projections a subject declares, and NO OTHER — keyed by `type` for a vertex, by
   * `edge_type` plus `direction` for one orientation of a relation. One shape for both, because
   * there is one vocabulary.
   */
  projections?: Array<{
    type?: string;
    edge_type?: string;
    direction?: Direction;
    scales: number[];
    sizes?: Array<{ scale: number; rows: string; tiles: string }>;
    tile_of?: Array<{ scale: number; dense_id: string; tile: string }>;
    addresses?: Array<{ scale: number; tile: number; path: string }>;
    files?: Array<{ scale: number; paths: string[] }>;
    refused?: Array<{ scale: number; message: string }>;
  }>;
}

const table = JSON.parse(readFileSync(join(CONFORMANCE, 'expected.json'), 'utf8')) as {
  base_join: { case: string; base: string; path: string; url: string };
  cases: Case[];
};

/** Every manifest under a case root, keyed the way a host that fetched them would key them. */
function manifestFiles(root: string): Record<string, string> {
  const files: Record<string, string> = {};
  for (const entry of readdirSync(root, { recursive: true, withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith('.yml')) continue;
    const path = join(entry.parentPath, entry.name);
    files[relative(root, path)] = readFileSync(path, 'utf8');
  }
  return files;
}

describe('the conformance corpus', () => {
  it('declares a pyramid somewhere, so the projection assertions are not an empty loop', () => {
    // Its own non-vacuity check, and it earned one: the `levels` case was deleted from
    // `expected.json` by a `git checkout` of a file that was not yet in the index, and every level
    // assertion in all three harnesses ran over an empty list and stayed green — the cases with no
    // pyramid carry every other count.
    const addresses = table.cases.flatMap((c) =>
      (c.projections ?? []).flatMap((p) => p.addresses ?? []),
    );
    expect(addresses.length).toBeGreaterThanOrEqual(4);
    // And a scale above 1 somewhere: a table of payloads passes against a reader that never
    // learned a coarser projection exists.
    expect(addresses.some((a) => a.scale > 1)).toBe(true);
  });

  it('has cases', () => {
    expect(table.cases.length).toBeGreaterThan(0);
  });

  for (const expected of table.cases) {
    const root = join(CONFORMANCE, expected.root);

    // **`async` because the door is.** The addressing is the shallowest rung of `openCorpus` now,
    // so resolving costs an `await` — and this suite resolves at COLLECTION time, to generate a
    // test per subject of the corpus rather than per row of a static table. Vitest awaits a suite
    // factory, which is what makes that legal; the alternative is deriving the subject list from
    // `expected.json` instead of from the corpus, and a table that decided which subjects to check
    // would stop being a table the corpus is checked against.
    describe(expected.name, async () => {
      if (expected.resolve_throws) {
        it('refuses a manifest that addresses nothing', async () => {
          await expect(openCorpus('', { manifestFiles: manifestFiles(root) })).rejects.toThrow(
            expected.resolve_throws,
          );
        });
        return;
      }

      const corpus: CorpusAddressing = await openCorpus('', {
        manifestFiles: manifestFiles(root),
      });

      it('reads the container off the manifest', () => {
        // The one thing about a corpus a reader cannot work out: working it out means listing a
        // directory, and there is no listing over HTTP. Every path below is what it decides.
        expect(corpus.container).toBe(expected.container);
      });

      it('resolves the vertex types the table declares', () => {
        expect(
          corpus.types.map((t) => ({
            type: t.type,
            prefix: t.prefix,
            chunk_size: t.chunkSize,
            shift: Number(t.shift),
          })),
        ).toEqual(expected.types);
      });

      it('resolves the edge types, and only the orientations the manifest publishes', () => {
        expect(
          corpus.edges.map((e) => ({
            edge_type: e.edgeType,
            src_type: e.srcType,
            dst_type: e.dstType,
            prefix: e.prefix,
            directions: [...e.directions],
            adjacencies: e.directions.map((d) => {
              const a = e.adjacency(d)!;
              return {
                direction: d,
                prefix: a.prefix,
                column: a.column,
                chunk_size: a.chunkSize,
                shift: Number(a.shift),
              };
            }),
          })),
        ).toEqual(expected.edges);
      });

      if ((expected.tile_of ?? []).length > 0) {
        it('shifts a dense_id into the tile the table names', () => {
          for (const v of expected.tile_of!) {
            expect(corpus.vertexType(v.type).tileOf(BigInt(v.dense_id))).toBe(BigInt(v.tile));
          }
        });
      }

      if ((expected.addresses ?? []).length > 0) {
        it(
          expected.on_disk
            ? 'composes the addresses in the table, and every one names a file on disk'
            : 'composes the addresses in the table',
          () => {
            for (const address of expected.addresses!) {
              const url =
                address.kind === 'vertex'
                  ? corpus.vertexType(address.type).tileUrl(address.tile)
                  : corpus.edges
                      .find((e) => e.edgeType === address.edge_type)!
                      .adjacency(address.direction!)!
                      .tileUrl(address.tile);
              expect(url).toBe(address.path);
              if (expected.on_disk) expect(existsSync(join(root, url))).toBe(true);
            }
          },
        );
      }

      if ((expected.refused ?? []).length > 0) {
        it('refuses an address the corpus does not publish', () => {
          for (const refused of expected.refused!) {
            const edge = corpus.edges.find((e) => e.edgeType === refused.edge_type)!;
            expect(edge.adjacency(refused.direction)).toBeNull();
            expect(edge.directions).not.toContain(refused.direction);
          }
        });
      }

      /**
       * Every subject of the corpus, in the ONE vocabulary — a vertex type and one orientation of
       * a relation answering the same four questions, because a level, an adjacency and a payload
       * are all a `path` and a `scale` now.
       */
      const subjects = [
        ...corpus.types.map((t) => ({
          name: t.type,
          direction: null as Direction | null,
          projections: t.projections,
          projection: (scale: number) => t.projection(scale),
          files: (scale: number) => t.projectionFiles(scale),
        })),
        ...corpus.edges.flatMap((e) =>
          (['src', 'dst'] as const).map((d) => ({
            name: e.edgeType,
            direction: d as Direction | null,
            projections: e.projections.filter((p) => p.direction === d),
            projection: (scale: number) => e.projection(scale, d),
            files: (scale: number) => e.projectionFiles(scale, d),
          })),
        ),
      ];
      const declared = (subject: (typeof subjects)[number]) =>
        (expected.projections ?? []).find((p) =>
          subject.direction === null
            ? p.type === subject.name
            : p.edge_type === subject.name && p.direction === subject.direction,
        );

      it('reports the projections the manifest declares, and no scale it invented', () => {
        // A reader that invented a scale would compose `l6/chunk0.parquet` against a corpus that
        // never wrote one — a 404 for a level the predicate over the payload answers. The default
        // the table does not spell out is a payload, or an orientation the corpus does not publish
        // at all; anything coarser is a scale the reader made up.
        for (const subject of subjects) {
          const want = declared(subject);
          expect(
            subject.projections.map((p) => p.scale),
            subject.direction === null ? subject.name : `${subject.name}/${subject.direction}`,
          ).toEqual(want?.scales ?? subject.projections.map(() => 1));
        }
      });

      for (const subject of subjects) {
        const want = declared(subject);
        if (want === undefined) continue;
        const label =
          subject.direction === null ? subject.name : `${subject.name}/${subject.direction}`;

        describe(`the projections of ${label}`, () => {
          if ((want.sizes ?? []).length > 0) {
            it('holds what the scale selects, counted', () => {
              for (const size of want.sizes!) {
                const found = subject.projection(size.scale)!;
                expect(found.rows, `scale ${size.scale}`).toBe(BigInt(size.rows));
                expect(found.tiles, `scale ${size.scale}`).toBe(BigInt(size.tiles));
                // No second `chunk_size` anywhere: the cut does not change with the scale.
                expect(found.chunkSize).toBe(subject.projection(1)!.chunkSize);
              }
            });
          }

          if ((want.tile_of ?? []).length > 0) {
            it('shifts a dense_id by the payload shift plus log2(scale)', () => {
              for (const v of want.tile_of!) {
                expect(subject.projection(v.scale)!.tileOf(BigInt(v.dense_id))).toBe(
                  BigInt(v.tile),
                );
              }
            });
          }

          if ((want.addresses ?? []).length > 0) {
            it('composes the addresses in the table', () => {
              for (const address of want.addresses!) {
                const url = subject.projection(address.scale)!.tileUrl(address.tile);
                expect(url).toBe(address.path);
                if (expected.on_disk) expect(existsSync(join(root, url))).toBe(true);
              }
            });
          }

          for (const set of want.files ?? []) {
            it(`enumerates scale ${set.scale}`, () => {
              expect([...subject.files(set.scale)]).toEqual(set.paths);
            });
          }

          for (const refused of want.refused ?? []) {
            it(`refuses to address scale ${refused.scale}, which nobody wrote`, () => {
              expect(() => subject.files(refused.scale)).toThrow(refused.message);
            });
          }
        });
      }

      for (const [index, expectation] of (expected.windows ?? []).entries()) {
        it(`window ${index}: ${expectation.directions.join('+')} over ${expectation.tiles.length} tile(s)`, () => {
          const got = corpus.tilesFor({
            type: expectation.type,
            tiles: expectation.tiles,
            directions: expectation.directions,
          });
          expect([...got.vertexUrls]).toEqual(expectation.vertex_urls);
          expect([...got.edgeUrls]).toEqual(expectation.edge_urls);
          expect(got.complete).toBe(expectation.complete);
          expect(
            got.gaps.map((g) => ({
              edge_type: g.edgeType,
              direction: g.direction,
              reason: g.reason,
            })),
          ).toEqual(expectation.gaps);
        });
      }

      for (const expectation of expected.throws ?? []) {
        it(`refuses vertex type ${expectation.vertex_type}`, () => {
          expect(() => corpus.vertexType(expectation.vertex_type)).toThrow(expectation.message);
        });
      }
    });
  }

  it('prepends the base and does nothing else to it', async () => {
    const { base, case: name, path, url } = table.base_join;
    const target = table.cases.find((c) => c.name === name)!;
    const corpus = await openCorpus(base, {
      manifestFiles: manifestFiles(join(CONFORMANCE, target.root)),
    });
    const tile = Number(path.replace(/^.*chunk(\d+)\.parquet$/, '$1'));
    expect(corpus.vertexType().tileUrl(tile)).toBe(url);
  });
});
