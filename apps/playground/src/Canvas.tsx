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
import { open } from '@fossil-lang/corpus';
import { GraphCanvas, type BoundedSource } from '@kanzo-tech/graph';
import { categoricalCapacity } from '@kanzo-tech/ui';
import { useCallback, useEffect, useMemo, useState } from 'react';

import type { Bench } from './bench.js';
import { readDeclaration } from './bound.js';
import { openCrossfilter, type Crossfilter, type CrossfilterCost } from './crossfilter.js';
import * as duck from './duckdb.js';
import { channelsFor, encodingFor, type Encoding } from '@fossil-lang/draw';
import { n } from './format.js';
import Histogram from './Histogram.js';
import { ModeComparison, START, StreamingDetail, WholeDetail, type Mode, type ModeLedger } from './Ledger.js';
import Mask from './Mask.js';
import { coordinatorFor } from './mosaic.js';
import PinFrame from './PinFrame.js';
import type { TileBox } from './stream.js';
import { corpusSource, type SliceCost } from './tiles.js';
import { wholeSource, type WholeCost } from './whole.js';

import './canvas.css';


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
          //
          // **Except when it did not pay.** A rectangle residency had already answered reached no
          // engine, so charging it again would make the comparison an argument about a read that
          // did not happen — which is the whole point of separating what is loaded from what is
          // drawn. `reads` is documented as *answers that reached DuckDB at all* and now means it;
          // `moves` counts every answer, resident or not, because a camera move is a camera move.
          bytes: was.bytes + (next.resident ? 0 : next.bytes),
          rows: next.marks + next.anchors,
          links: next.links,
          firstMs: was.reads === 0 ? next.ms : was.firstMs,
          lastMs: next.ms,
          moves: was.reads === 0 ? 0 : was.moves + 1,
          reads: was.reads + (next.resident ? 0 : 1),
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
   * **What this corpus declares it is drawn with**, read once and handed to all three readers.
   *
   * The block is per vertex type and lives in `vertex/<Type>.vertex.yml`, which `bench.ts` has
   * already fetched — so this is a scan of text in hand rather than a request, and it is
   * synchronous where `Corpus.types` is not. `undefined` for a corpus with no `channels:` key,
   * which is every corpus written before the field and which is why nothing below branches on it:
   * `encodingFor` and `categoricalOf` take the absence as *derive*, exactly as they did.
   *
   * Which type: the first the index names, taken off `bench.addressing` because that is the same
   * ordering `encodingFor` falls back to and the addressing already holds it. The encoding needs
   * the block to be computed, so the block cannot wait for the encoding to say which type it is.
   */
  const channels = useMemo(
    () => channelsFor(Object.values(bench.manifestTexts), bench.addressing.types[0]?.type ?? ''),
    [bench],
  );

  /**
   * The corpus, opened once — and as a PROMISE, which is what keeps this component unchanged.
   *
   * `open` is asynchronous and a `BoundedSource` is not: the query loop builds the source
   * synchronously and calls it later. Handing the promise to `corpusSource` means there is no
   * loading branch here and no state machine around the canvas — the source awaits it inside its
   * own members, once.
   *
   * **The reference call site for a host that can only sign.** The corpus is lent to the page's
   * engine under `bench/<count>`, every file behind a URL this host "signs" — a static server
   * needs no signature, so signing is composing `bench.base`, which is absolute for the reason
   * `bench.ts` records. The manifests are already in hand, so none is fetched again. Closing on
   * unmount gives the engine its files and views back.
   */
  const corpus = useMemo(
    () =>
      open(`bench/${bench.stamp.count}`, {
        engine: duck.engine,
        host: {
          sign: async (paths) => Object.fromEntries(paths.map((p) => [p, `${bench.base}/${p}`])),
        },
        manifestFiles: bench.manifestTexts,
      }),
    [bench],
  );
  useEffect(() => () => void corpus.then((c) => c.close()), [corpus]);

  const streaming: BoundedSource = useMemo(
    () => corpusSource({ corpus, boxes, channels, onCost, slots }),
    [channels, corpus, boxes, onCost, slots],
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
    () => wholeSource({ corpus, boxes, channels, query: duck.query, onCost: onWhole, slots }),
    [channels, corpus, boxes, onWhole, slots],
  );

  const source = mode === 'whole' ? baseline : streaming;

  /**
   * **What this corpus is drawn with**, read off it rather than written down here.
   *
   * Three string literals used to stand where this does — `fill="cluster_id"` on the renderer,
   * `field="birth_year"` on the chart, and the crossfilter view's four columns — each of them a
   * fact about the bench corpus inside a component that claims to read any corpus. `encoding.ts`
   * carries the derivation and the argument for which half of the artefact answers which question;
   * what is left here is the wiring.
   *
   * State and not a `useMemo`, because both inputs arrive late: the payload's vocabulary is one
   * `DESCRIBE` per type inside `open`, so it is behind the same promise the sources await.
   * The declaration is not — `bench.indexText` is the manifest as it was served — and it is read
   * here anyway, in the same effect, so there is one place where the two halves meet.
   */
  const [encoding, setEncoding] = useState<Encoding | null>(null);

  useEffect(() => {
    let live = true;
    void (async () => {
      const open = await corpus;
      if (!live) return;
      const declaration = readDeclaration(bench.indexText);
      setEncoding(
        encodingFor({
          types: open.types,
          channels,
          quasiIdentifiers:
            declaration.state === 'declared' ? declaration.bound.quasiIdentifiers : [],
        }),
      );
    })();
    return () => {
      live = false;
    };
  }, [bench, channels, corpus]);

  /**
   * The crossfilter, opened once the corpus is — one coordinator, over the engine already booted.
   *
   * `null` until the view exists, which is what `Histogram`'s `ready` gates on: a client connected
   * before `CREATE VIEW` would issue its domain query against a name DuckDB does not have.
   *
   * The count comes off the manifest rather than a `count(*)`, because it is the mask's LENGTH and
   * the mask is indexed by `dense_id` — so what it needs is the id space, which is what
   * `vertex_count` declares, and not how many rows a scan happens to find.
   */
  const [crossfilter, setCrossfilter] = useState<Crossfilter | null>(null);
  const [mask, setMask] = useState<Uint8Array | null>(null);
  const [xfCost, setXfCost] = useState<CrossfilterCost | null>(null);

  useEffect(() => {
    if (encoding === null) return;
    let live = true;
    let opened: Crossfilter | null = null;
    void (async () => {
      const open = await corpus;
      if (!live) return;
      opened = await openCrossfilter({
        coordinator: coordinatorFor(),
        corpus: open,
        type: encoding.type,
        count: encoding.count,
        columns: encoding.columns,
        onMask: (next, cost) => {
          if (!live) return;
          setMask(next);
          setXfCost(cost);
        },
      });
      if (!live) {
        opened.destroy();
        return;
      }
      setCrossfilter(opened);
    })();
    return () => {
      live = false;
      opened?.destroy();
      setCrossfilter(null);
      setMask(null);
    };
  }, [corpus, encoding]);

  return (
    <div className="can">
      <h2>the corpus, panned</h2>
      <p className="str-note">
        Drag to pan, scroll to zoom. Every camera move asks the source for the tiles the new
        rectangle touches and for nothing else — the same footer boxes the ledger above is reading,
        with a camera on them instead of a slider. Position is <code>x</code>/<code>y</code> and
        colour is the categorical this corpus carries — <code>{encoding?.fill ?? '…'}</code>,{' '}
        <em>read off the payload</em> rather than named here, because a manifest declares a
        property&apos;s name and its type and nothing about what to draw with it. Both are written
        by the layout pass. The colour is{' '}
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

      {/*
        * The renderer is not mounted before the encoding is known, and the wait costs nothing.
        *
        * `fill` is a property of the REQUEST in `@kanzo-tech/graph`'s contract — *a source says
        * where the bytes are; a request says what I want to draw* — so the query loop puts whatever
        * this prop holds into every slice it asks for. Mounted first and told afterwards, the
        * opening rectangle would be asked for twice: once with no column and once with one, which
        * is two different residency keys for one camera position and two `reads` on the ledger
        * beside it for a corpus that was read once. Nothing can be answered before the corpus
        * opens anyway — every `slice` awaits it inside the source — so gating here delays no query.
        */}
      <div className="can-surface">
        {encoding === null ? (
          <div className="can-waiting">
            <p className="str-dim">reading what this corpus is drawn with…</p>
          </div>
        ) : (
          <GraphCanvas source={source} fill={encoding.fill ?? undefined} onFailure={onFailure}>
            <PinFrame />
            <Mask mask={mask} />
          </GraphCanvas>
        )}
      </div>

      {failure && <p className="str-bad">{failure}</p>}

      <div className="xf">
        {crossfilter === null || encoding === null ? (
          <div className="xf-chart xf-waiting">
            <p className="str-dim">opening the crossfilter over this corpus…</p>
          </div>
        ) : encoding.brush === null ? (
          /*
           * A corpus whose payload carries no numeric column outside the address, the position and
           * the colour. The canvas still draws and the crossfilter still exists; what is absent is
           * the second client, and saying so is more honest than binning `dense_id`.
           */
          <div className="xf-chart xf-waiting">
            <p className="str-dim">
              this corpus carries no numeric column a histogram is the right form for, so there is
              one client on the coordinator and nothing to cross
            </p>
          </div>
        ) : (
          <Histogram
            coordinator={coordinatorFor()}
            field={encoding.brush}
            filter={crossfilter.filter}
            ready
          />
        )}
        <dl className="str-ledger xf-ledger">
          <div>
            <dt>survivors</dt>
            <dd>
              {xfCost ? n(xfCost.survivors) : '—'}{' '}
              <span className="str-dim">of {xfCost ? n(xfCost.population) : '—'}</span>
            </dd>
          </div>
          <div>
            <dt>mask built in</dt>
            <dd>{xfCost ? `${xfCost.maskMs.toFixed(1)} ms` : '—'}</dd>
          </div>
          <div>
            <dt>engines</dt>
            <dd>
              1 <span className="str-dim">coordinator on the app&apos;s own connection</span>
            </dd>
          </div>
        </dl>
      </div>

      <p className="str-note">
        <strong>The chart and the canvas are two clients of one Mosaic coordinator</strong>, and the
        coordinator runs on the same DuckDB-WASM the corpus is read through — not a second one.
        `@uwdata/mosaic-core` names <code>@duckdb/duckdb-wasm@1.33.1-dev57.0</code> as an exact
        dependency and this app pins <code>1.32.0</code>; the root manifest overrides mosaic&apos;s
        copy down, and what makes that safe is measured rather than hoped:{' '}
        <code>wasmConnector</code> takes a pre-existing instance and connection, and given both it
        never reaches <code>initDatabase()</code>, the only path that fetches a bundle and spawns a
        worker. The whole surface it then uses is{' '}
        <code>con.useUnsafe((bindings, conn) =&gt; bindings.runQuery(conn, sql))</code>, and{' '}
        <code>dist/types/src/parallel/async_connection.d.ts</code> is byte-identical between the two
        versions. <strong>The built bundle carries one <code>duckdb-*.wasm</code> asset</strong>,
        which <code>scripts/verify-one-engine.mjs</code> asserts rather than assumes.
      </p>

      <p className="str-note">
        <strong>What the crossfilter actually crosses.</strong> The chart brushes{' '}
        <code>{encoding?.brush ?? '…'}</code>, and which column that is <em>is read rather than
        chosen</em>: the payload says which of its columns a binned histogram is the right form for
        — numeric, and not the address, the position, the identity or the colour — and the vertex
        manifest&apos;s <code>channels:</code> block says which of those the writer means, by
        declaring it <em>quantitative</em>. Where a corpus declares none,{' '}
        <code>graph.graph.yml</code> still ranks them by naming one a{' '}
        <em>quasi-identifier</em> under the declared k-anonymity bound — which is how this was
        answered before the block existed, and is what a corpus written then still gets. The bytes
        decide the set and the declarations order it; no half is asked another&apos;s question. So
        the distribution on screen is the generalised one: the writer refused to seal a manifest
        whose data did not reach the bound, and this is what reached it.
        The two clients are different <em>kinds</em> on purpose.
        The chart&apos;s <code>x</code> is a column, so its brush publishes an interval the database
        evaluates directly. The canvas&apos;s is not: what is drawn is a rectangle-and-level answer
        from the door, decimated by <code>dense_id % 2^k</code> and cut at a mark budget, and no{' '}
        <code>WHERE</code> over columns describes <em>that</em>. So it goes the other way, through{' '}
        <code>IdSetClient</code> — ids out, a mask over the resident tiles — which is the adapter
        that exists for exactly this arrangement.{' '}
        <strong>Its cost is the length of that id list</strong>, and the ledger prints it: an
        unbrushed histogram returns one id per vertex in the corpus, which is the real ceiling on
        the approach. What would remove it is the door carrying the filtered column back beside{' '}
        <code>categories</code>, so the mask became a comparison per <em>drawn</em> vertex —
        twenty thousand rather than a million — and no id crossed the boundary at all.
      </p>

      <p className="str-note">
        <strong>The bars are all the same height, and that is the fixture rather than the chart.</strong>{' '}
        <code>apps/corpus/guards/fixture.mjs</code> assigns <code>birth_year</code> uniformly, so the
        million vertices fall 50,000 to a bin across twenty bins of two years — measured, not
        assumed. A flat histogram is the correct picture of a flat column, and the thing worth
        watching is not the bars but the canvas underneath them: because the layout pass placed
        vertices by community rather than by birth year, brushing a range fades a{' '}
        <em>scattered</em> subset rather than a region, which is what says the two encodings are
        independent. The bin width is snapped to a round number for a measured reason — 40 distinct
        years cut into a flat 28 bins alternates two-years-and-one, and draws a sawtooth that is an
        artefact of the divisor.
      </p>

      <ModeComparison mode={mode} ledgers={ledgers} />

      {ledgers.whole.failure && (
        <p className="str-bad">
          <strong>load everything:</strong> {ledgers.whole.failure}
        </p>
      )}

      {mode === 'whole' && whole && <WholeDetail whole={whole} />}

      {mode === 'streaming' && cost && <StreamingDetail cost={cost} />}

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
