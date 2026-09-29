/**
 * Iceberg's expressions, for a tile: `Filter` as data, bound to a matrix's columns, evaluated
 * against a tile's published statistics by the inclusive and the strict metrics evaluators, and
 * what is left of it rendered as SQL for the one engine that runs it.
 *
 * The references are PyIceberg's `pyiceberg/expressions/visitors.py` — `rewrite_not`,
 * `_InclusiveMetricsEvaluator`, `_StrictMetricsEvaluator` and the residual evaluator — and the
 * departures are the ones a tile forces, each named where it is taken.
 */

import { CorpusReadError, ident, lit } from './sql.js';

/**
 * A rectangle in the corpus's own coordinates. `w` and `h` extend from `x`/`y`.
 *
 * **Half-open on the far edge** — `x <= v.x < x + w`, and the same in `y`. It matters at exactly
 * one rectangle and it is the one a caller reaches for first: a box clamped to
 * {@link Corpus.extent} excludes the vertices sitting *on* the maximum, which is 17 of a million on
 * the bench corpus. A caller framing the whole extent nudges the far edge past it.
 */
export interface Box {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

/** The rectangle `rows` and `frame` select with, until step 5 retires both. */
export const boxOf = ({ x, y, w, h }: Box): string =>
  `x >= ${x} AND x < ${x + w} AND y >= ${y} AND y < ${y + h}`;

/** A value a filter compares a column against. */
export type Literal = number | bigint | string | boolean;

/**
 * **Iceberg's expression vocabulary, spelled as data** — `EqualTo`, `In`, `IsNull`, `And`, `Or`,
 * `Not` — so it crosses a worker boundary as it is. `StartsWith` and `IsNaN` are left out until
 * something asks.
 *
 * **Nulls are SQL's**: a comparison with a null is not a match, so `x != 5` does not select a row
 * whose `x` is null, and neither does `not (x = 5)`. **NaN is Iceberg's**: it is not a literal —
 * binding refuses one — and no comparison matches a NaN value, so `x > 5` selects none of them
 * although DuckDB orders NaN above every number; the rendered SQL says so explicitly. `!=` and
 * `not in` do match a NaN, as they do in both.
 */
export type Filter =
  | { readonly column: string; readonly op: '=' | '!=' | '<' | '<=' | '>' | '>='; readonly value: Literal }
  | { readonly column: string; readonly op: 'in' | 'not in'; readonly values: readonly Literal[] }
  | { readonly column: string; readonly op: 'is null' | 'not null' }
  | { readonly and: readonly Filter[] }
  | { readonly or: readonly Filter[] }
  | { readonly not: Filter };

/**
 * What a column's values are, as far as a bound is concerned. `float` is the one with a NaN the
 * statistics do not count; `other` has no bounds at all.
 */
export type ColumnKind = 'integer' | 'float' | 'other';

/** One tile's statistics — Iceberg's four, as the tile manifest publishes them. */
export interface Metrics {
  readonly recordCount: number;
  readonly nullValueCounts: Readonly<Record<string, number>>;
  readonly lowerBounds: Readonly<Record<string, number>>;
  readonly upperBounds: Readonly<Record<string, number>>;
}

type Comparison = '=' | '!=' | '<' | '<=' | '>' | '>=';
type Op = Comparison | 'in' | 'not in' | 'is null' | 'not null';

/** A predicate on one column, bound: its column resolved to a kind and NOT pushed past it. */
interface Leaf {
  readonly column: string;
  readonly kind: ColumnKind;
  readonly op: Op;
  readonly values: readonly Literal[];
}

/** A filter bound to a set of columns, with every `not` rewritten away. */
export type Bound =
  | { readonly leaf: Leaf }
  | { readonly and: readonly Bound[] }
  | { readonly or: readonly Bound[] }
  | boolean;

const NEGATED: Record<Op, Op> = {
  '=': '!=',
  '!=': '=',
  '<': '>=',
  '<=': '>',
  '>': '<=',
  '>=': '<',
  in: 'not in',
  'not in': 'in',
  'is null': 'not null',
  'not null': 'is null',
};

/** Every column a filter names, in the order it names them. */
export function columnsOf(filter: Filter): string[] {
  if ('and' in filter) return filter.and.flatMap(columnsOf);
  if ('or' in filter) return filter.or.flatMap(columnsOf);
  if ('not' in filter) return columnsOf(filter.not);
  return [filter.column];
}

/**
 * **Bind a filter to the columns a matrix carries** — Iceberg's `UnboundPredicate.bind(schema)`,
 * with `rewrite_not` folded in: a `not` is pushed to the leaves and inverts each, so the evaluators
 * below never see one.
 *
 * @throws {CorpusReadError} for a column the matrix does not carry, a NaN or non-finite number, or
 *   a literal whose type the column cannot hold — before anything is planned, as Iceberg binds.
 */
export function bind(filter: Filter, columns: ReadonlyMap<string, ColumnKind>): Bound {
  const walk = (node: Filter, negated: boolean): Bound => {
    if ('not' in node) return walk(node.not, !negated);
    if ('and' in node || 'or' in node) {
      const conjunction = 'and' in node !== negated;
      const children = ('and' in node ? node.and : node.or).map((child) => walk(child, negated));
      return conjunction ? { and: children } : { or: children };
    }
    const kind = columns.get(node.column);
    if (kind === undefined) {
      throw new CorpusReadError(
        `the filter names ${node.column}, which is not a column here — ` +
          `the columns are ${[...columns.keys()].join(', ')}`,
      );
    }
    const values = 'values' in node ? node.values : 'value' in node ? [node.value] : [];
    for (const value of values) literalFor(node.column, kind, value);
    return { leaf: { column: node.column, kind, op: negated ? NEGATED[node.op] : node.op, values } };
  };
  return walk(filter, false);
}

function literalFor(column: string, kind: ColumnKind, value: Literal): void {
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw new CorpusReadError(
      `${String(value)} is not a literal (${column}): a NaN matches no comparison, so a filter ` +
        'that means one names it by what it is, and an infinity is not a value a column holds',
    );
  }
  const numeric = typeof value === 'number' || typeof value === 'bigint';
  if (kind !== 'other' && !numeric) {
    throw new CorpusReadError(`${column} holds numbers, and the filter compares it with ${JSON.stringify(value)}`);
  }
}

/** `a` against `b`, exactly, whether either is a `bigint` or a `number`. */
function compare(a: number | bigint, b: number | bigint): number {
  if (typeof a === typeof b) return a < b ? -1 : a > b ? 1 : 0;
  const [big, num, sign] = typeof a === 'bigint' ? [a, b as number, 1] : [b as bigint, a as number, -1];
  const floor = BigInt(Math.floor(num));
  const order = big < floor ? -1 : big > floor ? 1 : Number.isInteger(num) ? 0 : 1;
  return order * sign;
}

type Stat = { lower?: number; upper?: number; nulls?: number; rows: number };

function statOf(leaf: Leaf, metrics: Metrics): Stat {
  const stat: Stat = { rows: metrics.recordCount };
  const nulls = metrics.nullValueCounts[leaf.column];
  if (nulls !== undefined) stat.nulls = nulls;
  if (leaf.kind !== 'other') {
    const lower = metrics.lowerBounds[leaf.column];
    const upper = metrics.upperBounds[leaf.column];
    if (lower !== undefined) stat.lower = lower;
    if (upper !== undefined) stat.upper = upper;
  }
  return stat;
}

const numeric = (values: readonly Literal[]): (number | bigint)[] =>
  values.filter((v): v is number | bigint => typeof v === 'number' || typeof v === 'bigint');

/**
 * **Might a row of this tile match?** — `_InclusiveMetricsEvaluator`. A tile is dropped only when
 * its statistics prove no row can, and a column with no statistics proves nothing.
 *
 * A float's bounds leave NaN out, as Parquet's do, and that is safe here and only here: no
 * comparison this evaluator prunes on matches a NaN.
 */
function mightMatch(leaf: Leaf, metrics: Metrics): boolean {
  const s = statOf(leaf, metrics);
  const allNull = s.nulls !== undefined && s.nulls === s.rows;
  switch (leaf.op) {
    case 'is null':
      return s.nulls === undefined || s.nulls > 0;
    case 'not null':
      return !allNull;
    case '!=':
    case 'not in':
      return !allNull;
    default:
      break;
  }
  if (allNull) return false;
  const [v] = leaf.values;
  const { lower, upper } = s;
  switch (leaf.op) {
    case '<':
      return lower === undefined || compare(lower, v as number) < 0;
    case '<=':
      return lower === undefined || compare(lower, v as number) <= 0;
    case '>':
      return upper === undefined || compare(upper, v as number) > 0;
    case '>=':
      return upper === undefined || compare(upper, v as number) >= 0;
    case '=':
      return (
        leaf.kind === 'other' ||
        ((lower === undefined || compare(lower, v as number) <= 0) &&
          (upper === undefined || compare(upper, v as number) >= 0))
      );
    case 'in':
      return (
        leaf.values.length > 0 &&
        (leaf.kind === 'other' ||
          numeric(leaf.values).some(
            (x) =>
              (lower === undefined || compare(lower, x) <= 0) &&
              (upper === undefined || compare(upper, x) >= 0),
          ))
      );
  }
  return true;
}

/**
 * **Must every row of this tile match?** — `_StrictMetricsEvaluator`, which PyIceberg's planner
 * does not use and this one does: it is what makes a residual `null`.
 *
 * **A float never strictly matches a comparison that excludes NaN**, because the manifest counts
 * no NaN — Parquet's footer does not, and the manifest publishes the footer's numbers and nothing
 * else. So a box over `x` and `y` keeps its residual on every tile, and the engine re-evaluates a
 * range the bounds already settle. What would reverse it is a `nan_value_counts` the writer
 * computes, which is Iceberg's fifth statistic and the one Parquet does not carry.
 */
function mustMatch(leaf: Leaf, metrics: Metrics): boolean {
  const s = statOf(leaf, metrics);
  if (leaf.op === 'is null') return s.nulls !== undefined && s.nulls === s.rows;
  if (leaf.op === 'not null') return s.nulls === 0;
  if (s.nulls !== 0 || s.lower === undefined || s.upper === undefined) return false;
  const { lower, upper } = s;
  const outside = (x: number | bigint) => compare(x, lower) < 0 || compare(x, upper) > 0;
  if (leaf.op === '!=') return outside(leaf.values[0] as number);
  if (leaf.op === 'not in') return numeric(leaf.values).length === leaf.values.length && numeric(leaf.values).every(outside);
  if (leaf.kind === 'float') return false;
  const [v] = leaf.values;
  switch (leaf.op) {
    case '<':
      return compare(upper, v as number) < 0;
    case '<=':
      return compare(upper, v as number) <= 0;
    case '>':
      return compare(lower, v as number) > 0;
    case '>=':
      return compare(lower, v as number) >= 0;
    case '=':
      return compare(lower, v as number) === 0 && compare(upper, v as number) === 0;
    case 'in':
      return compare(lower, upper) === 0 && numeric(leaf.values).some((x) => compare(x, lower) === 0);
  }
  return false;
}

/**
 * **What is left of a filter on one tile** — Iceberg's residual: `true` where the tile's statistics
 * prove every row matches, `false` where they prove none can, and otherwise the part they could not
 * settle, with every settled leaf folded away.
 */
export function residual(bound: Bound, metrics: Metrics): Bound {
  if (typeof bound === 'boolean') return bound;
  if ('leaf' in bound) {
    if (!mightMatch(bound.leaf, metrics)) return false;
    return mustMatch(bound.leaf, metrics) ? true : bound;
  }
  const conjunction = 'and' in bound;
  const kept: Bound[] = [];
  for (const child of conjunction ? bound.and : bound.or) {
    const left = residual(child, metrics);
    if (left === !conjunction) return left;
    if (left !== conjunction) kept.push(left);
  }
  if (kept.length === 0) return conjunction;
  if (kept.length === 1) return kept[0]!;
  return conjunction ? { and: kept } : { or: kept };
}

/** A bound filter back as the data a caller wrote — what a task's `residual` is. */
export function filterOf(bound: Bound): Filter | null {
  if (bound === true) return null;
  if (bound === false) return { or: [] };
  if ('and' in bound) return { and: bound.and.map((b) => filterOf(b) ?? { and: [] }) };
  if ('or' in bound) return { or: bound.or.map((b) => filterOf(b) ?? { and: [] }) };
  const { column, op, values } = bound.leaf;
  if (op === 'in' || op === 'not in') return { column, op, values };
  if (op === 'is null' || op === 'not null') return { column, op };
  return { column, op, value: values[0]! };
}

function literal(kind: ColumnKind, value: Literal): string {
  if (typeof value === 'string') return lit(value);
  if (typeof value === 'boolean') return value ? 'TRUE' : 'FALSE';
  // Doubles, stated: a float column is widened to `DOUBLE` beside it, so the engine compares the
  // same two numbers the planner compared and a bound sitting on the literal is judged alike.
  return kind === 'float' ? `${String(value)}::DOUBLE` : String(value);
}

/**
 * **A bound filter as DuckDB SQL** — the one place a filter becomes text, and the text is ours: a
 * caller's string never reaches it. A float column is compared as a `DOUBLE`, which is how the
 * planner compared its bounds, and `>`/`>=` on one exclude NaN, which DuckDB would otherwise count
 * as greater than every number.
 */
export function sqlOf(bound: Bound): string {
  if (bound === true) return 'TRUE';
  if (bound === false) return 'FALSE';
  if ('and' in bound) return `(${bound.and.map(sqlOf).join(' AND ')})`;
  if ('or' in bound) return `(${bound.or.map(sqlOf).join(' OR ')})`;
  const { column, kind, op, values } = bound.leaf;
  const name = kind === 'float' ? `${ident(column)}::DOUBLE` : ident(column);
  switch (op) {
    case 'is null':
      return `${ident(column)} IS NULL`;
    case 'not null':
      return `${ident(column)} IS NOT NULL`;
    case 'in':
    case 'not in':
      if (values.length === 0) return op === 'in' ? 'FALSE' : `${ident(column)} IS NOT NULL`;
      return `${name} ${op === 'in' ? 'IN' : 'NOT IN'} (${values.map((v) => literal(kind, v)).join(', ')})`;
    case '>':
    case '>=':
      if (kind === 'float') {
        return `(${name} ${op} ${literal(kind, values[0]!)} AND NOT isnan(${ident(column)}))`;
      }
      return `${name} ${op} ${literal(kind, values[0]!)}`;
    case '!=':
      return `${name} <> ${literal(kind, values[0]!)}`;
    default:
      return `${name} ${op} ${literal(kind, values[0]!)}`;
  }
}
