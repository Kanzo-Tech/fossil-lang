/**
 * A filter as data, bound to one table's columns and rendered as the `WHERE` of one statement.
 *
 * The vocabulary is Iceberg's expressions — `EqualTo`, `In`, `IsNull`, `And`, `Or`, `Not` — spelled
 * as data so it crosses a worker boundary as it is, plus a box over the table's `position`. Pruning
 * is DuckDB's: the predicate reaches `read_parquet`, and the row groups whose footer statistics
 * exclude it are not read.
 */

import type { Position, Property } from './manifest.js';
import { CorpusReadError, ident, lit } from './sql.js';

/** A value a filter compares a column against. */
export type Literal = number | bigint | string | boolean;

/**
 * Rows to keep.
 *
 * **Nulls are SQL's**: a comparison with a null is not a match, so `x != 5` does not select a row
 * whose `x` is null, and neither does `not (x = 5)`. **NaN is Iceberg's**: it is not a literal —
 * binding refuses one — and no comparison matches a NaN value, so `x > 5` selects none of them
 * although DuckDB orders NaN above every number. `!=` and `not in` do match a NaN, as they do in
 * both.
 *
 * `bbox` is `[x0, y0, x1, y1]` over the table's `position` columns, **closed on every edge**: a
 * vertex on the boundary is inside. A table with no `position` has no box.
 */
export type Filter =
  | { readonly column: string; readonly op: '=' | '!=' | '<' | '<=' | '>' | '>='; readonly value: Literal }
  | { readonly column: string; readonly op: 'in' | 'not in'; readonly values: readonly Literal[] }
  | { readonly column: string; readonly op: 'is null' | 'not null' }
  | { readonly and: readonly Filter[] }
  | { readonly or: readonly Filter[] }
  | { readonly not: Filter }
  | { readonly bbox: readonly [number, number, number, number] };

/** What a column's values are, as far as a comparison is concerned. */
type Kind = 'integer' | 'float' | 'other';

type Op = '=' | '!=' | '<' | '<=' | '>' | '>=' | 'in' | 'not in' | 'is null' | 'not null';

interface Leaf {
  readonly column: string;
  readonly kind: Kind;
  readonly op: Op;
  readonly values: readonly Literal[];
}

/** A filter bound to a table, with every `not` pushed to the leaves and every box expanded. */
export type Bound = { readonly leaf: Leaf } | { readonly and: readonly Bound[] } | { readonly or: readonly Bound[] };

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

function kindOf(type: string): Kind {
  if (/^u?int(8|16|32|64)$/.test(type)) return 'integer';
  if (type === 'float' || type === 'double' || type === 'float32' || type === 'float64') return 'float';
  return 'other';
}

/** The columns a filter is bound against: a table's declared properties, and its position. */
export interface Columns {
  readonly table: string;
  readonly properties: readonly Property[];
  readonly position: Position | undefined;
}

/**
 * **Bind a filter to a table** — Iceberg's `bind(schema)`, with `rewrite_not` folded in.
 *
 * @throws {CorpusReadError} for a column the table does not declare, a NaN or non-finite number, a
 *   literal the column cannot hold, or a box over a table with no position — before any statement.
 */
export function bind(filter: Filter, columns: Columns): Bound {
  const kinds = new Map(columns.properties.map((p) => [p.name, kindOf(p.type)]));
  const leaf = (column: string, op: Op, values: readonly Literal[], negated: boolean): Bound => {
    const kind = kinds.get(column);
    if (kind === undefined) {
      throw new CorpusReadError(
        `the filter names ${column}, which ${columns.table} does not declare — its columns are ` +
          `${[...kinds.keys()].join(', ')}`,
      );
    }
    for (const value of values) literalFor(column, kind, value);
    return { leaf: { column, kind, op: negated ? NEGATED[op] : op, values } };
  };
  const walk = (node: Filter, negated: boolean): Bound => {
    if ('not' in node) return walk(node.not, !negated);
    if ('and' in node || 'or' in node) {
      const conjunction = 'and' in node !== negated;
      const children = ('and' in node ? node.and : node.or).map((child) => walk(child, negated));
      return conjunction ? { and: children } : { or: children };
    }
    if ('bbox' in node) {
      const { position } = columns;
      if (position === undefined) {
        throw new CorpusReadError(`${columns.table} declares no position, so it has no box to filter by`);
      }
      const [x0, y0, x1, y1] = node.bbox;
      if (![x0, y0, x1, y1].every(Number.isFinite) || x0 > x1 || y0 > y1) {
        throw new CorpusReadError(`bbox ${JSON.stringify(node.bbox)} is not [x0, y0, x1, y1] with x0 ≤ x1 and y0 ≤ y1`);
      }
      const sides = [
        leaf(position.x, '>=', [x0], negated),
        leaf(position.x, '<=', [x1], negated),
        leaf(position.y, '>=', [y0], negated),
        leaf(position.y, '<=', [y1], negated),
      ];
      return negated ? { or: sides } : { and: sides };
    }
    const values = 'values' in node ? node.values : 'value' in node ? [node.value] : [];
    return leaf(node.column, node.op, values, negated);
  };
  return walk(filter, false);
}

function literalFor(column: string, kind: Kind, value: Literal): void {
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw new CorpusReadError(
      `${String(value)} is not a literal (${column}): a NaN matches no comparison, and an infinity ` +
        'is not a value a column holds',
    );
  }
  if (kind !== 'other' && typeof value !== 'number' && typeof value !== 'bigint') {
    throw new CorpusReadError(`${column} holds numbers, and the filter compares it with ${JSON.stringify(value)}`);
  }
}

function literal(value: Literal): string {
  if (typeof value === 'string') return lit(value);
  if (typeof value === 'boolean') return value ? 'TRUE' : 'FALSE';
  return String(value);
}

/**
 * **A bound filter as DuckDB SQL.** The column is compared as it is stored, never cast, because a
 * cast on the column is what keeps DuckDB from pruning a row group by its statistics; `>`/`>=` on a
 * float exclude NaN, which DuckDB would otherwise count as greater than every number.
 */
export function sqlOf(bound: Bound): string {
  if ('and' in bound) return bound.and.length === 0 ? 'TRUE' : `(${bound.and.map(sqlOf).join(' AND ')})`;
  if ('or' in bound) return bound.or.length === 0 ? 'FALSE' : `(${bound.or.map(sqlOf).join(' OR ')})`;
  const { column, kind, op, values } = bound.leaf;
  const name = ident(column);
  switch (op) {
    case 'is null':
      return `${name} IS NULL`;
    case 'not null':
      return `${name} IS NOT NULL`;
    case 'in':
    case 'not in':
      if (values.length === 0) return op === 'in' ? 'FALSE' : `${name} IS NOT NULL`;
      return `${name} ${op === 'in' ? 'IN' : 'NOT IN'} (${values.map(literal).join(', ')})`;
    case '>':
    case '>=':
      return kind === 'float'
        ? `(${name} ${op} ${literal(values[0]!)} AND NOT isnan(${name}))`
        : `${name} ${op} ${literal(values[0]!)}`;
    case '!=':
      return `${name} <> ${literal(values[0]!)}`;
    default:
      return `${name} ${op} ${literal(values[0]!)}`;
  }
}
