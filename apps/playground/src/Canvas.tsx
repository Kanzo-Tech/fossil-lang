/**
 * The million vertices, drawn — and drawn by panning, which is the whole claim.
 *
 * The ledger beside this canvas says a 10% window opens 17 of 245 tiles in six `Range` requests
 * and 1,466 kB. This is the same arithmetic with a camera on it: every drag re-asks, the answer is
 * the tiles the new rectangle touches, and the corpus is never held. A reader who drags across the
 * whole graph has seen a million vertices in a tab that never held more than twenty thousand of
 * them.
 *
 * ## What is fossil's and what is kanzo's
 *
 * `@kanzo-tech/graph` owns the renderer — cosmos.gl's lifetime, the query loop that follows the
 * camera, the buffers a look implies — and `src/tiles.ts` owns the answer. The seam between them
 * is `BoundedSource`, which is one method. Nothing in this file knows how a tile is addressed and
 * nothing in `@kanzo-tech/graph` knows what a corpus is.
 *
 * ## The simulation is OFF, and that is the correct default rather than a cautious one
 *
 * A bounded source hands back the coordinates its NEXT spatial query is expressed in. A force
 * would move the points out from under their own index, and the camera would drift away from the
 * thing it is addressing within one frame. The layout is the compiler's output and so is its
 * geometry: if a placement is wrong it is wrong upstream, and it is fixed by recompiling.
 */
import { GraphCanvas, type BoundedSource } from '@kanzo-tech/graph';
import { categoricalCapacity } from '@kanzo-tech/ui';
import { useCallback, useMemo, useState } from 'react';

import type { Bench } from './bench.js';
import * as duck from './duckdb.js';
import type { TileBox } from './stream.js';
import { corpusSource, type SliceCost } from './tiles.js';

import './canvas.css';

const KB = (bytes: number) => `${(bytes / 1024).toFixed(0)} kB`;
const n = (value: number) => value.toLocaleString();

export interface CanvasProps {
  bench: Bench;
  /** The footer, already bought. The source answers `extent` and picks tiles out of these. */
  boxes: readonly TileBox[];
}

/**
 * The canvas, its source, and a ledger of what the last frame cost.
 *
 * `useMemo` over the source and not `useState`: it is a value derived from the corpus and the
 * boxes, and rebuilding it is what the query loop watches for. Rebuilt on every render it would
 * re-register the payloads and re-ask `total()` once per frame.
 */
export default function Canvas({ bench, boxes }: CanvasProps) {
  const [cost, setCost] = useState<SliceCost | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const onCost = useCallback((next: SliceCost) => setCost(next), []);
  const onFailure = useCallback((message: string) => setFailure(message), []);

  /**
   * How many colours the palette has, asked of the document rather than assumed.
   *
   * `--chart-capacity` is a `:root` token of `@kanzo-tech/ui`'s stylesheet and its themes disagree
   * about it, so the number that decides which ordinals get a colour is not a constant this app
   * may write down. Read off `documentElement` and not off the canvas: the source is built before
   * the element exists, and nothing here scopes an override.
   */
  const slots = useMemo(() => categoricalCapacity(document.documentElement), []);

  const source: BoundedSource = useMemo(
    () =>
      corpusSource({
        addressing: bench.addressing,
        boxes,
        register: duck.registerUrl,
        query: duck.query,
        onCost,
        slots,
      }),
    [bench, boxes, onCost, slots],
  );

  return (
    <div className="can">
      <h2>the corpus, panned</h2>
      <p className="str-note">
        Drag to pan, scroll to zoom. Every camera move asks the source for the tiles the new
        rectangle touches and for nothing else — the same footer boxes the ledger above is reading,
        with a camera on them instead of a slider. Position is <code>x</code>/<code>y</code> and
        colour is <code>cluster_id</code>, both written by the layout pass. The colour is{' '}
        <em>folded</em>: this corpus has 128 communities and the palette has {slots} slots, so two
        communities can share one — which is what a categorical palette is. Unfolded they do not
        share a colour, they share <em>the</em> colour: everything past the last slot resolves to
        the muted token, and 937,496 of the million vertices came back one grey. The simulation is
        off on purpose: a force would move the points out from under the coordinates the next query
        is expressed in.
      </p>

      <div className="can-surface">
        <GraphCanvas source={source} fill="cluster_id" onFailure={onFailure} />
      </div>

      {failure && <p className="str-bad">{failure}</p>}

      {cost && (
        <dl className="str-ledger">
          <div>
            <dt>tiles this frame</dt>
            <dd>
              {cost.tiles} of {cost.ofTiles}
            </dd>
          </div>
          <div>
            <dt>in runs</dt>
            <dd>
              {cost.runs} <span className="str-dim">≈ {KB(cost.bytes)}</span>
            </dd>
          </div>
          <div>
            <dt>drawn</dt>
            <dd>
              {n(cost.marks)} of {n(cost.matched)}
              {cost.anchors > 0 && <span className="str-dim"> +{n(cost.anchors)} anchors</span>}
            </dd>
          </div>
          <div>
            <dt>links</dt>
            <dd>{n(cost.links)}</dd>
          </div>
          <div>
            <dt>answered in</dt>
            <dd>{cost.ms.toFixed(0)} ms</dd>
          </div>
        </dl>
      )}

      <p className="str-note">
        <strong>drawn</strong> is the sample and <strong>of</strong> is what the window holds — over
        the limit the source strides across <code>dense_id</code> rather than taking the front of
        it, so what is drawn is a picture of the window and not of one corner of it. That works
        because the ids ascend along the Morton curve, which is the same property that makes the
        tiles contiguous. <strong>anchors</strong> are the far ends of edges the sample did not
        draw — real vertices at their real coordinates, never painted, and free. Two ways to be
        one, and neither costs a request: outside the rectangle, or inside it and not in the
        sample. A tile is {n(bench.stamp.chunkSize)} rows of a Morton-ordered relation, so the
        vertices just outside the rectangle are usually in bytes already fetched. The byte figure
        beside{' '}
        <strong>in runs</strong> is the footer&apos;s arithmetic over the same tiles, not a weighed
        response: DuckDB issues these requests from inside its Worker, where this thread cannot see
        them. The ledger above weighs its own.
      </p>

      <p className="str-note">
        <strong>links is 0 on this corpus, and that is the corpus rather than the reader.</strong>{' '}
        A bounded reader can only draw an edge whose far end it has a position for, and it has
        positions for the tiles it fetched. This demo corpus is{' '}
        <code>apps/corpus/guards/fixture.mjs</code>, whose edges are a <em>ring over the
        pre-layout index</em> plus one chord per vertex inside its cluster — so after the Morton
        renumbering the destinations are scattered across the whole id space. Measured over its{' '}
        {n(bench.stamp.edges)} edges: <strong>2,096 (0.10%)</strong> have both ends in the same
        tile and <strong>61,244 (3.06%)</strong> within eight tiles of each other. Nothing is
        broken and nothing here would fix it: an edge to a vertex 200 tiles away is a request for
        another tile, which is the read a windowed corpus exists not to make. A corpus whose
        adjacency follows its geometry draws them; this one is a spatial index with a random graph
        laid over it, and it is honest for the panel to say so rather than to fetch the corpus to
        hide it.
      </p>
    </div>
  );
}
