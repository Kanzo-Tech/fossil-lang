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
import { openCorpus } from '@fossil-lang/corpus';
import { GraphCanvas, useGraphContext, type BoundedSource } from '@kanzo-tech/graph';
import { categoricalCapacity } from '@kanzo-tech/ui';
import { useCallback, useEffect, useMemo, useState } from 'react';

import type { Bench } from './bench.js';
import * as duck from './duckdb.js';
import type { TileBox } from './stream.js';
import { corpusSource, type SliceCost } from './tiles.js';
import { wholeSource, type WholeCost } from './whole.js';

import './canvas.css';

const KB = (bytes: number) => `${(bytes / 1024).toFixed(0)} kB`;
const MB = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`;
const n = (value: number) => value.toLocaleString();

export interface CanvasProps {
  bench: Bench;
  /** The footer, already bought. The source answers `extent` and picks tiles out of these. */
  boxes: readonly TileBox[];
}

/** Which of the two paths is mounted. */
type Mode = 'streaming' | 'whole';

/**
 * The four numbers the two modes are compared on, and nothing that only one of them has.
 *
 * A tiled viewer is judged against *just load the array*, and it is judged on these: what it read,
 * what it is holding, how long until there was a picture, and what a camera move costs. Both
 * ledgers below fold into this so the comparison is one table rather than two panels a reader has
 * to hold in their head — and both keep their own detail underneath, because the interesting
 * numbers are not the same on the two sides.
 *
 * **`bytes` is cumulative and `rows` is not**, which is the asymmetry the whole comparison turns
 * on. The windowed path reads again on every move, so its byte figure only means anything summed;
 * it holds one window, so its row figure only means anything per frame. The baseline reads once
 * and holds everything, so both of its numbers are the same number for the rest of the session.
 */
interface ModeLedger {
  bytes: number;
  rows: number;
  links: number;
  firstMs: number;
  lastMs: number;
  moves: number;
  /**
   * Answers that reached DuckDB at all.
   *
   * Not a count of SQL statements — the two modes issue different numbers of those for one answer,
   * and adding them up would compare nothing. This is the number that separates the paths: the
   * windowed one is every answer, the baseline is one and then never again.
   */
  reads: number;
  failure: string | null;
}

/** The empty ledger, so a mode that has answered nothing yet is a shape rather than a `null`. */
const START: ModeLedger = {
  bytes: 0,
  rows: 0,
  links: 0,
  firstMs: 0,
  lastMs: 0,
  moves: 0,
  reads: 0,
  failure: null,
};

/**
 * **Pin the frame**, which is the one line between a camera that refines and one that breaks.
 *
 * cosmos.gl rescales the positions it is handed to fill its space, deriving the map from the extent
 * of *that upload*. With the simulation off — the correct default here — `rescalePositions` resolves
 * to on, and `setPointPositions` is called with no second argument. So every answer rebuilds the map
 * from itself, and `screenToSpacePosition`, which the query loop reads the camera with, starts
 * speaking a coordinate system the corpus never wrote. The rectangle the source is asked about is
 * then a function of the PREVIOUS answer.
 *
 * Measured over the bench corpus, two routes to the same camera rectangle: straight in, the source
 * is asked about x[−981, 347] and draws 8,395; wandering in and out and back, it is asked about
 * x[−3719, −1022], which is off the corpus, and draws **nothing**. Pinned, both ask x[−253, 1355]
 * and draw the same 18,308. `scripts/verify-properties.mjs` is that measurement and
 * `/frame-probe.html` is where the arithmetic was checked against the library's own `getScaleX()`.
 *
 * A component rather than a prop because `@kanzo-tech/graph` publishes no passthrough for cosmos.gl
 * config — `useGraphContext` is the seam it does publish, and a child of `GraphCanvas` is where it
 * can be read. **The fix belongs upstream**: a renderer whose source hands back the coordinates its
 * next query is expressed in must not move them, and the day `GraphCanvas` says so this goes.
 */
function PinFrame() {
  const { getGraph } = useGraphContext();
  useEffect(() => {
    let live = true;
    // The renderer is built in an effect of its own and there is no ready signal to await from out
    // here, so this waits for the instance rather than assuming it. A macrotask and not a frame:
    // `requestAnimationFrame` does not fire in a tab that is not visible, and this must not depend
    // on being watched.
    const pin = () => {
      if (!live) return;
      const graph = getGraph();
      if (graph === null) {
        setTimeout(pin, 16);
        return;
      }
      graph.setConfigPartial({ rescalePositions: false });
    };
    pin();
    return () => {
      live = false;
    };
  }, [getGraph]);
  return null;
}

/**
 * The canvas, its source, and a ledger of what the last frame cost.
 *
 * `useMemo` over the source and not `useState`: it is a value derived from the corpus and the
 * boxes, and rebuilding it is what the query loop watches for. Rebuilt on every render it would
 * re-open the corpus and re-ask `total()` once per frame.
 */
export default function Canvas({ bench, boxes }: CanvasProps) {
  const [mode, setMode] = useState<Mode>('streaming');
  const [cost, setCost] = useState<SliceCost | null>(null);
  const [whole, setWhole] = useState<WholeCost | null>(null);
  const [ledgers, setLedgers] = useState<Record<Mode, ModeLedger>>({ streaming: START, whole: START });
  const [failure, setFailure] = useState<string | null>(null);

  /**
   * Fold each mode's own ledger into the shared four, and keep the detail beside it.
   *
   * A functional update rather than a read of `ledgers`, because these callbacks are `useMemo`
   * dependencies of the sources: one that closed over the current ledger would rebuild the source
   * on every answer, and rebuilding the baseline's source is re-loading the corpus.
   */
  const onCost = useCallback((next: SliceCost) => {
    setCost(next);
    setLedgers((prior) => {
      const was = prior.streaming;
      return {
        ...prior,
        streaming: {
          // Cumulative: the windowed path pays again on every move, so a per-frame figure would
          // flatter it against a baseline that pays once.
          bytes: was.bytes + next.bytes,
          rows: next.marks + next.anchors,
          links: next.links,
          firstMs: was.reads === 0 ? next.ms : was.firstMs,
          lastMs: next.ms,
          moves: was.reads,
          reads: was.reads + 1,
          failure: null,
        },
      };
    });
  }, []);

  const onWhole = useCallback((next: WholeCost) => {
    setWhole(next);
    setLedgers((prior) => ({
      ...prior,
      whole: {
        bytes: next.bytes,
        rows: next.rows,
        links: next.links,
        firstMs: next.loadMs,
        // A camera move that answers from memory. Zero is a legitimate reading of
        // `performance.now()` twice with a returned reference between them, and that IS the claim.
        lastMs: next.moves === 0 ? next.loadMs : next.ms,
        moves: next.moves,
        reads: 1,
        failure: next.failure,
      },
    }));
  }, []);

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

  /**
   * The corpus, opened once — and as a PROMISE, which is what keeps this component unchanged.
   *
   * `openCorpus` is asynchronous and a `BoundedSource` is not: the query loop builds the source
   * synchronously and calls it later. Handing the promise to `corpusSource` means there is no
   * loading branch here and no state machine around the canvas — the source awaits it inside its
   * own members, once. `bench.base` is absolute for the reason `bench.ts` records: DuckDB resolves
   * a URL inside its Worker, where a relative one names the wrong directory.
   */
  const corpus = useMemo(() => openCorpus(bench.base, { query: duck.query }), [bench]);

  const streaming: BoundedSource = useMemo(
    () => corpusSource({ corpus, boxes, onCost, slots }),
    [corpus, boxes, onCost, slots],
  );

  /**
   * The baseline, built beside it and **not rebuilt when the mode changes**.
   *
   * Both sources exist from the first render and neither is torn down by the toggle, which is what
   * makes the comparison a comparison: switching back to `whole` after a pan re-mounts a source
   * that has already loaded, so the ledger keeps saying what the load cost rather than paying it
   * again. It also means the corpus is loaded at most once per session, and only if the reader
   * asks for it — nothing here is loaded eagerly.
   */
  const baseline: BoundedSource = useMemo(
    () => wholeSource({ corpus, boxes, query: duck.query, onCost: onWhole, slots }),
    [corpus, boxes, onWhole, slots],
  );

  const source = mode === 'whole' ? baseline : streaming;

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
        is expressed in. <strong>The renderer&apos;s own rescale is off for the same reason</strong>{' '}
        — see <code>PinFrame</code> above — and without it two routes to the same camera rectangle
        asked this source about two different rectangles, one of them off the corpus entirely.
      </p>

      <div className="str-actions" role="group" aria-label="which path draws the canvas">
        <button type="button" aria-pressed={mode === 'streaming'} onClick={() => setMode('streaming')}>
          streaming
        </button>
        <button type="button" aria-pressed={mode === 'whole'} onClick={() => setMode('whole')}>
          load everything
        </button>
        <span className="str-dim">
          {mode === 'streaming'
            ? 'the tiles the rectangle touches, on every move'
            : 'the whole payload once, then the renderer does the culling'}
        </span>
      </div>

      <div className="can-surface">
        <GraphCanvas source={source} fill="cluster_id" onFailure={onFailure}>
          <PinFrame />
        </GraphCanvas>
      </div>

      {failure && <p className="str-bad">{failure}</p>}

      <table className="can-compare">
        <caption>
          the same corpus, both ways — the mode in bold is the one on screen, and a column with no
          numbers is a mode nobody has asked for yet
        </caption>
        <thead>
          <tr>
            <th scope="col"> </th>
            <th scope="col" className={mode === 'streaming' ? 'on' : undefined}>
              streaming
            </th>
            <th scope="col" className={mode === 'whole' ? 'on' : undefined}>
              load everything
            </th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <th scope="row">bytes read</th>
            <td>
              {MB(ledgers.streaming.bytes)}{' '}
              <span className="str-dim">over {n(ledgers.streaming.reads)}</span>
            </td>
            <td>
              {MB(ledgers.whole.bytes)} <span className="str-dim">once</span>
            </td>
          </tr>
          <tr>
            <th scope="row">rows held</th>
            <td>
              {n(ledgers.streaming.rows)} <span className="str-dim">+{n(ledgers.streaming.links)} links</span>
            </td>
            <td>
              {n(ledgers.whole.rows)} <span className="str-dim">+{n(ledgers.whole.links)} links</span>
            </td>
          </tr>
          <tr>
            <th scope="row">first paint</th>
            <td>{ledgers.streaming.firstMs.toFixed(0)} ms</td>
            <td>{ledgers.whole.firstMs.toFixed(0)} ms</td>
          </tr>
          <tr>
            <th scope="row">per camera move</th>
            <td>
              {ledgers.streaming.lastMs.toFixed(0)} ms{' '}
              <span className="str-dim">last of {n(ledgers.streaming.moves)}</span>
            </td>
            <td>
              {ledgers.whole.lastMs.toFixed(1)} ms{' '}
              <span className="str-dim">last of {n(ledgers.whole.moves)}</span>
            </td>
          </tr>
        </tbody>
      </table>

      {ledgers.whole.failure && (
        <p className="str-bad">
          <strong>load everything:</strong> {ledgers.whole.failure}
        </p>
      )}

      {mode === 'whole' && whole && (
        <dl className="str-ledger">
          <div>
            <dt>held in this tab</dt>
            <dd>{MB(whole.held)}</dd>
          </div>
          <div>
            <dt>columns read</dt>
            <dd>
              {MB(whole.bytes)} <span className="str-dim">dense_id x y cluster_id · src dst</span>
            </dd>
          </div>
          <div>
            <dt>queries the load issued</dt>
            <dd>
              {n(whole.queries)} <span className="str-dim">and no more</span>
            </dd>
          </div>
          <div>
            <dt>drawn</dt>
            <dd>
              {n(whole.rows)} <span className="str-dim">every one a mark, no anchors</span>
            </dd>
          </div>
          <div>
            <dt>links</dt>
            <dd>{n(whole.links)}</dd>
          </div>
          <div>
            <dt>this move</dt>
            <dd>
              {whole.ms.toFixed(1)} ms <span className="str-dim">move {n(whole.moves)}</span>
            </dd>
          </div>
        </dl>
      )}

      {mode === 'streaming' && cost && (
        <dl className="str-ledger">
          {cost.box && (
            <div>
              <dt>asked about</dt>
              <dd>
                x[{Math.round(cost.box.x)}, {Math.round(cost.box.x + cost.box.w)}] y[
                {Math.round(cost.box.y)}, {Math.round(cost.box.y + cost.box.h)}]
              </dd>
            </div>
          )}
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
          <div>
            <dt>addressed by</dt>
            <dd>
              {cost.addressed} <span className="str-dim">· read {cost.read}</span>
            </dd>
          </div>
        </dl>
      )}

      <p className="str-note">
        <strong>
          The two byte figures are not the same convention and must not be added up or subtracted.
        </strong>{' '}
        <em>streaming</em>&apos;s counts whole vertex <em>tiles</em> — every column of them,
        including <code>subject</code>, which is the widest one and is never drawn — and counts no
        adjacency file at all, because <code>ViewCost</code> reports the tiles the rectangle
        selected. <em>load everything</em>&apos;s counts the columns it actually projects, across
        the vertex payload <em>and</em> <code>by_source</code>. Neither is a weighed response:
        DuckDB reads from inside its Worker where this thread cannot see the requests, so both are
        footer arithmetic and both say so. What the comparison is honest about is the shape rather
        than the ratio — one number is paid once and the other is paid again on every move, and the{' '}
        <strong>bytes read</strong> row is cumulative for exactly that reason.
      </p>

      <p className="str-note">
        <strong>load everything is the baseline, not the alternative on offer.</strong> It reads the
        whole vertex payload and the whole <code>by_source</code> adjacency through the same one
        DuckDB connection, keeps them in four typed arrays indexed by <code>dense_id</code>, and
        after that ignores the viewport: every camera move is answered with the same{' '}
        <code>Slice</code> object, so React&apos;s own identity check means nothing is re-uploaded
        and panning is pure GPU. That is the picture a tiled reader is measured against and it is
        drawn here rather than described. It also has a declared ceiling — 256 MB of typed array —
        and crossing it is a refusal with the arithmetic attached, printed above in red, because a
        baseline that wedges the tab silently is not a measurement.
      </p>

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
        <strong>
          Almost every edge of this corpus is missing from every frame, and that is the corpus
          rather than the reader.
        </strong>{' '}
        A bounded reader can only draw an edge whose far end it has a position for, and it has
        positions for the tiles it fetched. This demo corpus is{' '}
        <code>apps/corpus/guards/fixture.mjs</code>, whose edges are a <em>ring over the
        pre-layout index</em> plus one chord per vertex inside its cluster — so after the Morton
        renumbering the destinations are scattered across the whole id space. Measured over its{' '}
        {n(bench.stamp.edges)} edges, at the four windows{' '}
        <code>scripts/verify-canvas.mjs</code> opens: the whole-corpus view draws{' '}
        <strong>62,024 (3.10%)</strong>, a 30% window <strong>22,309</strong>, a 10% window{' '}
        <strong>2,796</strong>, a 2% window <strong>77</strong>. This paragraph said{' '}
        <em>links is 0 on this corpus</em>, which was true of no window the panel opens and false
        by 62,024 of the one it opens with — the ledger four lines above was printing the
        counter-example the whole time. Nothing is broken and nothing here would fix it: an edge to
        a vertex 200 tiles away is a request for another tile, which is the read a windowed corpus
        exists not to make. A corpus whose adjacency follows its geometry draws them; this one is a
        spatial index with a random graph laid over it, and it is honest for the panel to say so
        rather than to fetch the corpus to hide it.
      </p>
    </div>
  );
}
