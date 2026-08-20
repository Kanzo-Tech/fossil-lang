import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { resolveCorpus, type Direction, type ResolvedCorpus } from '../src/index.js';

/**
 * The conformance corpus, executed against the published module.
 *
 * `apps/corpus/conformance/expected.json` is a table of addresses — what a reader must compose from
 * a manifest and what it must refuse to compose. This file is one of the two implementations that
 * run it; `apps/corpus/conformance/verify.mjs` is the other, in plain Node with no npm at all,
 * because the format's claim is that a reader who has never heard of this package can open the same
 * corpus. Neither implementation wrote the table.
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
  it('has cases', () => {
    expect(table.cases.length).toBeGreaterThan(0);
  });

  for (const expected of table.cases) {
    const root = join(CONFORMANCE, expected.root);

    describe(expected.name, () => {
      if (expected.resolve_throws) {
        it('refuses a manifest that addresses nothing', () => {
          expect(() => resolveCorpus({ manifestFiles: manifestFiles(root) })).toThrow(
            expected.resolve_throws,
          );
        });
        return;
      }

      const corpus: ResolvedCorpus = resolveCorpus({ manifestFiles: manifestFiles(root) });

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

      for (const [index, expectation] of (expected.windows ?? []).entries()) {
        it(`window ${index}: ${expectation.directions.join('+')} over ${expectation.tiles.length} tile(s)`, () => {
          const got = corpus.window({
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

  it('prepends the base and does nothing else to it', () => {
    const { base, case: name, path, url } = table.base_join;
    const target = table.cases.find((c) => c.name === name)!;
    const corpus = resolveCorpus({
      manifestFiles: manifestFiles(join(CONFORMANCE, target.root)),
      base,
    });
    const tile = Number(path.replace(/^.*chunk(\d+)\.parquet$/, '$1'));
    expect(corpus.vertexType().tileUrl(tile)).toBe(url);
  });
});
