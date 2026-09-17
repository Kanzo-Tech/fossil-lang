/**
 * The chart beside the canvas — the second client, without which none of this is a crossfilter.
 *
 * ## Which column it brushes, and why this file does not decide
 *
 * `field` is a prop, and what fills it is `@fossil-lang/draw` reading the corpus — this header used
 * to carry the reasoning as prose about `birth_year` and the bench corpus, which is an argument
 * performed once by a person and then true of one artefact. The rule it argued is the rule that is
 * now executed, in two halves that are asked different questions:
 *
 * - **the payload** says which columns a binned histogram is the right form for — *numeric, and
 *   not the address, the position, the identity, or the colour*, the last because brushing the
 *   colour is brushing the legend;
 * - **the vertex manifest's `channels:` block** says which of those the writer means, by declaring
 *   it `quantitative`. It is the only field in the format whose subject is the drawing, so it
 *   outranks the other one;
 * - **`graph.graph.yml`**, where no channel is declared, still says which is interesting by
 *   declaring it a quasi-identifier under the corpus's k-anonymity bound. That is a
 *   disclosure-control field answering an encoding question, which is what `channels:` exists to
 *   stop — and it stays as the fallback because a corpus written before the block has to keep
 *   drawing the chart it drew.
 *
 * On the bench corpus all of them land on `birth_year`: it is declared as the channel `age`, and it
 * is also one of `quasi_identifiers: Person.birth_year Person.postcode` under a `k` of 5. So the
 * column this chart brushes is one the corpus says is a re-identification risk, and the chart shows
 * the k-anonymised distribution — the generalisation is already in the bytes, because the writer
 * refused to seal a manifest whose data did not reach the bound.
 *
 * ## No plotting dependency, deliberately
 *
 * This is ~40 lines of SVG: rectangles on a linear scale and a drag that reads two x positions.
 * `@uwdata/vgplot` would draw it, and it is absent from `@kanzo-tech/mosaic` on purpose — «it is a
 * plotting library, it belongs to the charts in `@kanzo-tech/ui`, and its accidental presence on
 * this path is exactly the bug this package exists to end». Adding it here to avoid writing a
 * `<rect>` would re-introduce the dependency that package was split to remove, and it would arrive
 * with a second copy of `@uwdata/mosaic-core` behind it if its range ever drifted. The point of the
 * chart is that a second client exists, not that it is a good chart.
 *
 * ## What it publishes
 *
 * A `clauseInterval` over `field` — an interval in *data space*, because unlike the canvas
 * this client's `x` really is a column. That is the asymmetry `IdSetClient`'s header describes from
 * the other side, and having both on one coordinator is what makes this a crossfilter rather than
 * a filter: the histogram narrows the canvas, and the canvas's own selection (were it to publish
 * one) would narrow the histogram, through the same `Selection.crossfilter()`.
 */
import { MosaicClient, Query, clauseInterval, column, type Selection } from '@kanzo-tech/mosaic';
// Straight from mosaic-sql rather than through the barrel: `@kanzo-tech/mosaic` re-exports the
// conversation — coordinator, clients, clauses — and `Query` with it, but not the expression
// helpers, and inventing a string where a typed node exists is how a `count(*)` ends up quoted.
// It is a declared dependency of this app precisely because it is a required peer of that package,
// so this is the same resolved copy and not a second one.
import { count, floor, least, sql } from '@uwdata/mosaic-sql';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { XF_VIEW } from './crossfilter.js';

/**
 * The most bars, not the number of them. Enough to see a shape, few enough that one is a
 * comfortable drag target.
 */
const MAX_BINS = 28;

/**
 * A bin width that is a round number, at most `span / MAX_BINS`… rounded UP to 1, 2, 5 × 10^k.
 *
 * **This is not decoration, it is the difference between a histogram and a comb.** `birth_year` on
 * the bench corpus runs 1950–1989: forty distinct integers. Cut into a flat 28 bins the width is
 * 1.393, so bins alternately capture two years and one — measured, the counts came back
 * 75,000 / 50,000 / 75,000 / 50,000 — and the chart shows a regular sawtooth that is an artefact of
 * the divisor and nothing about the data. Snapped, the width is 2, every bar holds exactly two
 * years, and the shape on screen is the distribution.
 *
 * Nice-number ticking rather than a special case for integers, because the same failure appears for
 * any column whose values are quantised — a price in cents, a rating in halves — and asking DuckDB
 * for the column type would answer a narrower question than the one that matters.
 */
function niceWidth(span: number): number {
  const raw = span / MAX_BINS;
  if (!(raw > 0)) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / magnitude; // in [1, 10)
  const step = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10;
  return step * magnitude;
}

const WIDTH = 520;
const HEIGHT = 132;
const PAD = { top: 8, right: 8, bottom: 22, left: 40 };

interface Bar {
  x0: number;
  x1: number;
  count: number;
}

/** The domain and the bars, or `null` before the first answer. */
interface Shape {
  lo: number;
  hi: number;
  bars: Bar[];
  max: number;
}

/**
 * A binned count over one numeric column, as a Mosaic client.
 *
 * `prepare()` is where the domain is settled, and it is a separate query on purpose: the extent has
 * to be the WHOLE column's, once, or the bars would rescale under the reader's own brush every time
 * they narrowed it. The client protocol has a hook for exactly this — «potentially issuing one or
 * more queries to gather data or metadata needed prior to `query` calls» — so the domain is bought
 * before the first binned query rather than inferred from it.
 *
 * `filterStable` is `true`: the bin domain is fixed by `prepare`, so a change to `filterBy` does not
 * move it, and the coordinator may pre-aggregate. That is not a micro-optimisation — it is the
 * declaration that makes a crossfilter's repeated re-query cheap, and it would be a lie if the
 * domain came from the filtered rows.
 */
class HistogramClient extends MosaicClient {
  #column: string;
  #onShape: (shape: Shape | null) => void;
  #lo = 0;
  #hi = 1;
  /** The snapped bin width, and how many of them cover the domain. Both settled by `prepare`. */
  #width = 1;
  #bins = MAX_BINS;

  constructor(field: string, filterBy: Selection, onShape: (shape: Shape | null) => void) {
    super(filterBy);
    this.#column = field;
    this.#onShape = onShape;
  }

  get filterStable(): boolean {
    return true;
  }

  override async prepare(): Promise<void> {
    const rows = (await this.coordinator?.query(
      `SELECT min(${this.#column}) AS lo, max(${this.#column}) AS hi FROM ${XF_VIEW}`,
      { type: 'json' },
    )) as { lo: number; hi: number }[] | undefined;
    const first = rows?.[0];
    if (first && Number.isFinite(Number(first.lo)) && Number.isFinite(Number(first.hi))) {
      this.#lo = Number(first.lo);
      // A degenerate domain — one distinct value — would make every bin width zero and every bar
      // infinitely tall. One unit of width is the honest picture of a column with no spread.
      this.#hi = Number(first.hi) > this.#lo ? Number(first.hi) : this.#lo + 1;
    }
    this.#width = niceWidth(this.#hi - this.#lo);
    // `ceil` and no `+ 1`: it already covers the domain, and the maximum value folds into the last
    // bin by the `least(…)` below. Adding one drew a permanently empty bar on the right — a span of
    // 39 at width 2 is TWENTY bins (1950–1990, the top one holding 1988–1989), not twenty-one.
    this.#bins = Math.max(1, Math.ceil((this.#hi - this.#lo) / this.#width));
    // The drawn domain is the bins', not the column's, or the last bar would overflow the axis.
    this.#hi = this.#lo + this.#bins * this.#width;
  }

  // `?? []` and not a non-null assertion: the coordinator passes `null` for «no filter», and an
  // empty `where` is exactly what that means to the query builder.
  override query(filter: Parameters<MosaicClient['query']>[0] = []) {
    const where = filter ?? [];
    // `least(…, bins - 1)` folds anything at the very top of the domain into the last bin rather
    // than giving it a bin of its own: `floor(span / width)` lands exactly on `bins` for the row
    // that defines the maximum, and one extra bar one row tall is a rendering of a rounding rule.
    //
    // The column name interpolates as raw SQL rather than as a literal — it is this component's own
    // `field` prop, not anything a reader typed — and the two numbers are what `prepare` settled.
    const bin = least(
      floor(sql`(${this.#column} - ${this.#lo}) / ${this.#width}`),
      this.#bins - 1,
    );
    return Query.from(XF_VIEW)
      .select({ bin, count: count() })
      .where(where)
      .groupby(bin);
  }

  override queryResult(data: unknown): this {
    const bins = column(data, 'bin').map(Number);
    const counts = column(data, 'count').map(Number);
    // Every bin, in order, and zero where the answer had no row: `GROUP BY` returns only the bins
    // that matched, and a brush that empties the middle of the domain would otherwise close the gap
    // and redraw the distribution as something it is not.
    const bars: Bar[] = Array.from({ length: this.#bins }, (_unused, i) => ({
      x0: this.#lo + i * this.#width,
      x1: this.#lo + (i + 1) * this.#width,
      count: 0,
    }));
    for (let i = 0; i < bins.length; i += 1) {
      const slot = bins[i];
      if (slot !== undefined && slot >= 0 && slot < this.#bins) bars[slot]!.count = counts[i] ?? 0;
    }
    this.#onShape({ lo: this.#lo, hi: this.#hi, bars, max: Math.max(1, ...bars.map((b) => b.count)) });
    return this;
  }
}

export interface HistogramProps {
  coordinator: { connect(client: MosaicClient): void; disconnect(client: MosaicClient): void };
  /** What the brush publishes into — the same `Selection` the canvas client is filtered by. */
  filter: Selection;
  /**
   * Which column. Derived from the corpus by `@fossil-lang/draw` and passed in, so that the one
   * module that decides what this app draws with decides this too.
   */
  field: string;
  /** Whether the relation is ready. The client is not connected before it is. */
  ready: boolean;
}

/**
 * The chart, and the drag that brushes it.
 *
 * The brush is two numbers and a pointer capture rather than a d3 selection: `onPointerDown`
 * records where in data space the drag started, `onPointerMove` records where it is, and release
 * publishes. Publishing on *move* as well is what makes it a crossfilter you can feel — the canvas
 * re-queries as the range grows — and the coordinator's own throttling is what keeps that from
 * being one query per pixel.
 */
export default function Histogram({ coordinator, field, filter, ready }: HistogramProps) {
  const [shape, setShape] = useState<Shape | null>(null);
  const [range, setRange] = useState<[number, number] | null>(null);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const dragFrom = useRef<number | null>(null);

  const client = useMemo(
    () => new HistogramClient(field, filter, setShape),
    [field, filter],
  );

  useEffect(() => {
    if (!ready) return;
    coordinator.connect(client);
    return () => coordinator.disconnect(client);
  }, [client, coordinator, ready]);

  const plotW = WIDTH - PAD.left - PAD.right;
  const plotH = HEIGHT - PAD.top - PAD.bottom;

  /** Screen x → data x, through the same linear scale the bars are drawn with. */
  const dataAt = useCallback(
    (event: React.PointerEvent<SVGSVGElement>): number | null => {
      const svg = svgRef.current;
      if (svg === null || shape === null) return null;
      const box = svg.getBoundingClientRect();
      // The SVG is scaled by CSS to its container, so a client x has to come back through the
      // viewBox before it means anything in plot units.
      const x = ((event.clientX - box.left) / box.width) * WIDTH - PAD.left;
      const t = Math.min(1, Math.max(0, x / plotW));
      return shape.lo + t * (shape.hi - shape.lo);
    },
    [plotW, shape],
  );

  /** Publish, or retract. `null` clears the clause, which the resolver drops entirely. */
  const publish = useCallback(
    (next: [number, number] | null) => {
      filter.update(
        clauseInterval(field, next, { source: client, clients: new Set([client]) }),
      );
    },
    [client, field, filter],
  );

  const onDown = (event: React.PointerEvent<SVGSVGElement>) => {
    const at = dataAt(event);
    if (at === null) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    dragFrom.current = at;
    setRange([at, at]);
  };

  const onMove = (event: React.PointerEvent<SVGSVGElement>) => {
    const from = dragFrom.current;
    if (from === null) return;
    const at = dataAt(event);
    if (at === null) return;
    const next: [number, number] = from <= at ? [from, at] : [at, from];
    setRange(next);
    publish(next);
  };

  const onUp = (event: React.PointerEvent<SVGSVGElement>) => {
    const from = dragFrom.current;
    dragFrom.current = null;
    if (from === null || shape === null) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    const at = dataAt(event);
    // A click rather than a drag — under one bin wide — is a clear. It is the gesture people try
    // first when they want the filter gone, and leaving a one-pixel interval selected instead is
    // how a crossfilter appears to have broken.
    if (at === null || Math.abs(at - from) < (shape.bars[0] ? shape.bars[0].x1 - shape.bars[0].x0 : 1) / 2) {
      setRange(null);
      publish(null);
    }
  };

  const clear = () => {
    setRange(null);
    publish(null);
  };

  const xOf = (value: number) =>
    shape === null ? 0 : PAD.left + ((value - shape.lo) / (shape.hi - shape.lo)) * plotW;

  return (
    <div className="xf-chart">
      <div className="xf-head">
        <h3>
          <code>{field}</code> <span className="str-dim">— drag to brush, click to clear</span>
        </h3>
        {range && (
          <button type="button" onClick={clear}>
            clear {Math.round(range[0])}–{Math.round(range[1])}
          </button>
        )}
      </div>
      <svg
        ref={svgRef}
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        className="xf-svg"
        role="img"
        aria-label={`distribution of ${field}, brushable`}
        onPointerDown={onDown}
        onPointerMove={onMove}
        onPointerUp={onUp}
      >
        {shape === null ? (
          <text x={PAD.left} y={HEIGHT / 2} className="xf-empty">
            reading the column…
          </text>
        ) : (
          <>
            {shape.bars.map((bar) => {
              const h = (bar.count / shape.max) * plotH;
              const x = xOf(bar.x0);
              const w = Math.max(1, xOf(bar.x1) - x - 1);
              const inside =
                range === null || (bar.x1 > range[0] && bar.x0 < range[1]);
              return (
                <rect
                  key={bar.x0}
                  x={x}
                  y={PAD.top + plotH - h}
                  width={w}
                  height={h}
                  className={inside ? 'xf-bar' : 'xf-bar xf-bar-out'}
                />
              );
            })}
            {range && (
              <rect
                x={xOf(range[0])}
                y={PAD.top}
                width={Math.max(1, xOf(range[1]) - xOf(range[0]))}
                height={plotH}
                className="xf-brush"
              />
            )}
            <line
              x1={PAD.left}
              y1={PAD.top + plotH}
              x2={PAD.left + plotW}
              y2={PAD.top + plotH}
              className="xf-axis"
            />
            <text x={PAD.left} y={HEIGHT - 6} className="xf-tick">
              {Math.round(shape.lo)}
            </text>
            <text x={PAD.left + plotW} y={HEIGHT - 6} textAnchor="end" className="xf-tick">
              {Math.round(shape.hi)}
            </text>
          </>
        )}
      </svg>
    </div>
  );
}
