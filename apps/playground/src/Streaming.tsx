/**
 * The streaming panel: a million vertices, and a question that reads one percent of them.
 *
 * The five-row demo next door proves the loop runs. It cannot prove anything about the corpus
 * format, because at five rows there is one tile and every question reads all of it. This
 * panel is the other size: `scripts/bench-corpus.mjs` writes a million-vertex corpus into
 * `public/`, and dragging the window changes how much of it comes off the wire.
 *
 * What the reader should come away having SEEN, in this order:
 *
 *   1. **Every URL, before any request.** The manifests are three small YAML files; from them
 *      `resolveCorpus` names all 245 tiles synchronously. The panel prints the count and the
 *      microseconds it took, and lists the URLs a given window resolves to — before fetching.
 *   2. **The footer, bought once.** Which tiles a rectangle touches comes from the Parquet
 *      footer. It is read once per corpus and its cost is shown alongside the payload's, so
 *      the one thing that grows with N is visible as the one thing that grows with N.
 *   3. **The bytes.** One `Range` request per run of adjacent tiles, weighed in this thread,
 *      against the size of the whole corpus.
 *
 * Self-contained: its wiring is one element in `App.tsx`, it carries its own stylesheet, and
 * it shares nothing with the editor half beyond the DuckDB connection the app already had.
 */
import { useCallback, useEffect, useRef, useState } from 'react';

import { openBench, type Bench } from './bench.js';
import Canvas from './Canvas.js';
import * as duck from './duckdb.js';
import {
  FOOTER_SQL,
  extentOf,
  fetchRuns,
  runsOf,
  selectTiles,
  toTileBox,
  windowIn,
  type Rect,
  type StreamCost,
  type TileBox,
} from './stream.js';
import './streaming.css';

const MB = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(2)} MB`;
const KB = (bytes: number) => `${(bytes / 1024).toFixed(0)} kB`;
const pct = (a: number, b: number) => `${((a / b) * 100).toFixed(2)}%`;

/** The name DuckDB knows the payload by. Registered as a URL, not as bytes. */
const PAYLOAD = 'bench-tiles.parquet';

/**
 * Where the vertex payload is — **asked of the addressing rather than spelled here.**
 *
 * This panel used to compose `${bench.base}/vertex/Person/tiles.parquet` at two call sites, which
 * is a second implementation of the one thing `resolveCorpus` exists to do: it reads the type's
 * `prefix` out of the manifest and knows the container, so `vertex/Person/` and `tiles.parquet`
 * are its answers and not this file's. A corpus whose manifest put the type somewhere else would
 * have been addressed correctly by the URL list below and fetched from the wrong place by the two
 * lines above it.
 */
const payloadUrl = (bench: Bench) => bench.addressing.vertexType().tileUrl(0);

type State = 'opening' | 'addressed' | 'absent' | 'failed';

export interface StreamingProps {
  /** False until DuckDB has booted — the footer read needs it. */
  ready: boolean;
}

export default function Streaming({ ready }: StreamingProps) {
  const [state, setState] = useState<State>('opening');
  const [error, setError] = useState<string | null>(null);
  const [bench, setBench] = useState<Bench | null>(null);
  const [boxes, setBoxes] = useState<TileBox[] | null>(null);
  const [footerMs, setFooterMs] = useState(0);
  const [reading, setReading] = useState(false);
  const [fraction, setFraction] = useState(0.1);
  const [cost, setCost] = useState<StreamCost | null>(null);
  const [streaming, setStreaming] = useState(false);
  const probed = useRef(false);

  // Step one, on mount and unconditionally, because it costs three small YAML files and no
  // engine at all. Guarded by a ref: `StrictMode` mounts twice on purpose.
  useEffect(() => {
    if (probed.current) return;
    probed.current = true;
    (async () => {
      try {
        const b = await openBench();
        if (!b) setState('absent');
        else {
          setBench(b);
          setState('addressed');
        }
      } catch (cause) {
        setError(String(cause));
        setState('failed');
      }
    })();
  }, []);

  /**
   * Step two, and it is a BUTTON rather than an effect, for two reasons.
   *
   * The narrative one: everything above this point cost three YAML files, and everything below
   * it needs a Parquet reader. Making the reader an explicit act is what lets the panel put a
   * number on the difference instead of asserting there is one.
   *
   * The mechanical one: this runs on the same DuckDB connection the Run button uses, and
   * duckdb-wasm serialises per connection. A footer read that fires by itself on page load can
   * land underneath a run; one the reader asks for cannot arrive unbidden.
   */
  const readFooter = useCallback(async () => {
    if (!bench || reading) return;
    setReading(true);
    setError(null);
    try {
      await duck.registerUrl(PAYLOAD, payloadUrl(bench));
      const started = performance.now();
      const rows = await duck.query(FOOTER_SQL(PAYLOAD));
      setFooterMs(performance.now() - started);
      setBoxes(rows.map(toTileBox));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setReading(false);
    }
  }, [bench, reading]);

  const extent: Rect | null = boxes ? extentOf(boxes) : null;
  const rect = extent ? windowIn(extent, fraction) : null;
  const chosen = boxes && rect ? selectTiles(boxes, rect) : [];
  const runs = runsOf(chosen);

  // The addressing, computed for the current window — synchronously, in render, because that
  // is what "no engine, no request" means when it is true.
  const address = bench && chosen.length > 0
    ? bench.addressing.tilesFor({ tiles: chosen.map((t) => t.tile), directions: ['src'] })
    : null;

  const onStream = async () => {
    if (!bench || boxes === null) return;
    setStreaming(true);
    setError(null);
    try {
      setCost(await fetchRuns(payloadUrl(bench), runs, boxes.length, chosen.length));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setStreaming(false);
    }
  };

  if (state === 'absent') {
    return (
      <div className="str">
        <h2>streaming — no bench corpus</h2>
        <p className="str-note">
          The large corpus is a build artifact, not a checked-in one. Generate it with{' '}
          <code>node scripts/bench-corpus.mjs</code> (about five seconds, 56 MB into{' '}
          <code>public/bench/</code>, gitignored) and reload.
        </p>
      </div>
    );
  }

  if (!bench) {
    return (
      <div className="str">
        <h2>streaming</h2>
        <p className={state === 'failed' ? 'str-bad' : 'str-note'}>
          {state === 'failed' ? `failed: ${error}` : 'reading manifests…'}
        </p>
      </div>
    );
  }

  const header = (
    <>
      <h2>
        streaming — {bench.stamp.count.toLocaleString()} vertices, {bench.stamp.tiles} tiles,{' '}
        {MB(bench.stamp.bytes)}
      </h2>

      <p className="str-claim">
        The corpus is addressed. <strong>{bench.manifestCount} manifests, {KB(bench.manifestBytes)}</strong>{' '}
        fetched in {bench.fetchMs.toFixed(0)} ms, then <code>resolveCorpus</code> in{' '}
        <strong>{bench.resolveMs.toFixed(2)} ms</strong> — synchronous, no engine, no WASM. Every
        one of the {bench.stamp.tiles} tile URLs is now computable by arithmetic: a tile is{' '}
        <code>dense_id &gt;&gt; 12</code>. Nothing below has been requested yet.
      </p>
    </>
  );

  // Step two has not happened: the addressing is free, the footer is not, and the panel says
  // which is which by making the reader ask for the second one.
  if (!boxes || !extent || !rect) {
    return (
      <div className="str">
        {header}
        <div className="str-actions">
          <button onClick={readFooter} disabled={reading || !ready}>
            {reading ? 'reading the footer…' : 'Read the footer'}
          </button>
          <span className="str-dim">
            {ready ? 'one query, one time, through DuckDB' : 'waiting for the engine to boot'}
          </span>
        </div>
        {error && <p className="str-bad">{error}</p>}
        <p className="str-note">
          Which tiles a <em>rectangle</em> touches is the one question the arithmetic above cannot
          answer: it comes from the per-tile <code>x</code>/<code>y</code> statistics in the Parquet
          footer, and reading a footer needs a Parquet reader.{' '}
          <code>@fossil-lang/graph</code> deliberately carries none — the host has one. This is the
          only step on this panel that costs an engine, and it happens once per corpus.
        </p>
      </div>
    );
  }

  const payloadBytes = boxes.reduce((a, b) => a + b.bytes, 0);
  const askedBytes = runs.reduce((a, r) => a + r.bytes, 0);

  return (
    <div className="str">
      {header}

      <label className="str-window">
        <span>
          window — {(fraction * 100).toFixed(0)}% of each axis
        </span>
        <input
          type="range"
          min={1}
          max={100}
          value={Math.round(fraction * 100)}
          onChange={(e) => {
            setFraction(Number(e.target.value) / 100);
            setCost(null);
          }}
        />
      </label>

      <div className="str-map" role="img" aria-label={`${chosen.length} of ${boxes.length} tiles selected`}>
        {boxes.map((b) => {
          const sel = b.xlo <= rect!.xhi && b.xhi >= rect!.xlo && b.ylo <= rect!.yhi && b.yhi >= rect!.ylo;
          return <i key={b.tile} className={sel ? 'on' : ''} title={`tile ${b.tile} — ${KB(b.bytes)}`} />;
        })}
      </div>

      <dl className="str-ledger">
        <div>
          <dt>tiles opened</dt>
          <dd>
            {chosen.length} of {boxes.length}
          </dd>
        </div>
        <div>
          <dt>requests</dt>
          <dd>
            {runs.length} <span className="str-dim">runs of adjacent tiles</span>
          </dd>
        </div>
        <div>
          <dt>would ask for</dt>
          <dd>{KB(askedBytes)}</dd>
        </div>
        <div>
          <dt>of the payload</dt>
          <dd>{pct(askedBytes, payloadBytes)}</dd>
        </div>
      </dl>

      <p className="str-note">
        {chosen.length} tiles collapse into {runs.length} contiguous byte intervals, so the
        question costs {runs.length} HTTP requests and not {chosen.length}. That is the
        O(√n)-runs property of a Morton order, as a request count. Under{' '}
        <code>container: files</code> the same window would cost one request per tile — measured
        at 4× in <code>/docs/format/conventions/addressing</code>, for the same bytes.
      </p>

      <div className="str-actions">
        <button onClick={onStream} disabled={streaming || chosen.length === 0}>
          {streaming ? 'streaming…' : `Stream these ${runs.length} ranges`}
        </button>
        {cost && !cost.ranged && (
          <span className="str-bad">
            the server ignored Range and sent whole files — the byte count below is what really arrived
          </span>
        )}
      </div>

      {error && <p className="str-bad">{error}</p>}

      {cost && (
        <>
          <h2>off the wire</h2>
          <dl className="str-ledger">
            <div>
              <dt>requests</dt>
              <dd>
                {cost.requests} × <code>Range</code>
              </dd>
            </div>
            <div>
              <dt>bytes received</dt>
              <dd>{KB(cost.gotBytes)}</dd>
            </div>
            <div>
              <dt>of the corpus</dt>
              <dd>
                {pct(cost.gotBytes, bench.stamp.bytes)}{' '}
                <span className="str-dim">of {MB(bench.stamp.bytes)}</span>
              </dd>
            </div>
            <div>
              <dt>elapsed</dt>
              <dd>{cost.ms.toFixed(0)} ms</dd>
            </div>
          </dl>
          <p className="str-note">
            Weighed in this thread, from the responses themselves — not from the footer's
            arithmetic, which said {KB(cost.askedBytes)}. The two agreeing is the check.
          </p>
        </>
      )}

      <h2>the footer — bought once</h2>
      <p className="str-note">
        Which tiles a rectangle touches is not arithmetic: it comes from the per-tile{' '}
        <code>x</code>/<code>y</code> statistics in the Parquet footer, read once per corpus in{' '}
        <strong>{footerMs.toFixed(0)} ms</strong> and reused by every window above.{' '}
        <code>@fossil-lang/graph</code> carries no Parquet reader on purpose — the host has one,
        and here it is DuckDB. It is the one structure a reader holds that grows with N.
      </p>

      <h2>the URLs, before the requests</h2>
      <p className="str-note">
        {address
          ? `${address.vertexUrls.length} vertex URL${address.vertexUrls.length === 1 ? '' : 's'}, ${address.edgeUrls.length} edge URL${address.edgeUrls.length === 1 ? '' : 's'}${address.complete ? '' : ` — incomplete: ${address.gaps.map((g) => `${g.edgeType}/${g.direction} ${g.reason}`).join(', ')}`}`
          : 'no tiles selected'}
      </p>
      {address && (
        <ul className="str-urls">
          {[...address.vertexUrls, ...address.edgeUrls].map((u) => (
            <li key={u}>{u}</li>
          ))}
        </ul>
      )}
      <p className="str-note">
        One URL under <code>container: rowgroups</code>, because row group <em>k</em> IS tile{' '}
        <em>k</em> and a row group has no URL of its own — the address is the byte range, which
        is what the {runs.length} requests above carry. The list is what a reader would GET
        under <code>container: files</code>, unchanged.
      </p>

      {/*
        The same footer, with a camera on it instead of a slider.
        Below the ledger rather than above it because the ledger is the argument and this is the
        argument being true: a reader should have seen the byte count before seeing the picture it
        bought. It mounts only once the boxes exist, which is what keeps `Read the footer` the one
        explicit step that costs an engine.
      */}
      <Canvas bench={bench} boxes={boxes} />
    </div>
  );
}

