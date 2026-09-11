/**
 * What a frame cost, and the table that puts the two modes beside each other.
 *
 * Split out of `Canvas.tsx` unchanged. It is the one part of that panel that is about neither the
 * corpus nor the renderer: it is the **judgement**, and a second reader over a corpus that does not
 * fit on a GPU is judged on the same four numbers. `Canvas` keeps the state; this renders it.
 */
import type { SliceCost } from './tiles.js';
import type { WholeCost } from './whole.js';
import { KB, MB, n } from './format.js';


/** Which of the two paths is mounted. */
export type Mode = 'streaming' | 'whole';

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
export interface ModeLedger {
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
export const START: ModeLedger = {
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
 * The four numbers, both ways, as one table rather than two panels a reader holds in their head.
 */
export function ModeComparison({ mode, ledgers }: { mode: Mode; ledgers: Record<Mode, ModeLedger> }) {
  return (
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
  );
}

/** `load everything`'s own detail — the numbers only the baseline has. */
export function WholeDetail({ whole }: { whole: WholeCost }) {
  return (
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
  );
}

/** `streaming`'s own detail — the numbers only a windowed read has. */
export function StreamingDetail({ cost }: { cost: SliceCost }) {
  return (
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
            {cost.requests} <span className="str-dim">≈ {KB(cost.bytes)}</span>
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
          <dd>
            {cost.ms.toFixed(0)} ms{' '}
            <span className="str-dim">{cost.resident ? 'out of residency' : 'through the door'}</span>
          </dd>
        </div>
        <div>
          <dt>counted at</dt>
          <dd>level {cost.matchedAt}</dd>
        </div>
        {/*
          What is LOADED, beside what is drawn — the two numbers `/docs/design/camera` asks to be
          kept apart. `drawn` above is the answer on screen; this is every row held from every
          rectangle this camera has already visited, which is what an interim frame is assembled out
          of while the next answer is in flight.
        */}
        <div>
          <dt>held</dt>
          <dd>
            {n(cost.held)} <span className="str-dim">rows, across the rectangles already asked</span>
          </dd>
        </div>
      </dl>
  );
}
