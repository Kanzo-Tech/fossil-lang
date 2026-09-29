import { describe, expect, it } from 'vitest';

import {
  bind,
  filterOf,
  residual,
  sqlOf,
  type ColumnKind,
  type Filter,
  type Metrics,
} from '../src/expression.js';
import { CorpusReadError } from '../src/sql.js';

// Iceberg's metrics evaluators, held to their semantics on one tile at a time: `false` is a tile
// the statistics prove holds no match, `true` one they prove holds nothing else, and anything else
// is the residual a read still has to apply. Every expectation is stated against a tile written
// out below, never against the implementation's own output.

const COLUMNS = new Map<string, ColumnKind>([
  ['dense_id', 'integer'],
  ['year', 'integer'],
  ['x', 'float'],
  ['subject', 'other'],
]);

/** 64 rows: `dense_id` 64..127, `year` 1950..1990 with 3 nulls, `x` in [-1.5, 2.5], a string. */
const TILE: Metrics = {
  recordCount: 64,
  nullValueCounts: { dense_id: 0, year: 3, x: 0, subject: 0 },
  lowerBounds: { dense_id: 64, year: 1950, x: -1.5 },
  upperBounds: { dense_id: 127, year: 1990, x: 2.5 },
};

const on = (filter: Filter, tile: Metrics = TILE) => residual(bind(filter, COLUMNS), tile);

describe('binding', () => {
  it('refuses a column the matrix does not carry, naming the ones it does', () => {
    expect(() => bind({ column: 'mode', op: '=', value: 1 }, COLUMNS)).toThrow(CorpusReadError);
    expect(() => bind({ column: 'mode', op: '=', value: 1 }, COLUMNS)).toThrow(/dense_id, year, x, subject/);
  });

  it('refuses NaN and infinity as literals, and a string against a number', () => {
    expect(() => bind({ column: 'x', op: '<', value: Number.NaN }, COLUMNS)).toThrow(/NaN/);
    expect(() => bind({ column: 'x', op: '<', value: Infinity }, COLUMNS)).toThrow(CorpusReadError);
    expect(() => bind({ column: 'year', op: '=', value: '1990' }, COLUMNS)).toThrow(/numbers/);
  });

  it('pushes not to the leaves, as rewrite_not does', () => {
    const bound = bind({ not: { and: [{ column: 'year', op: '<', value: 1960 }, { column: 'x', op: 'is null' }] } }, COLUMNS);
    expect(filterOf(bound)).toEqual({
      or: [
        { column: 'year', op: '>=', value: 1960 },
        { column: 'x', op: 'not null' },
      ],
    });
    expect(filterOf(bind({ not: { not: { column: 'year', op: 'in', values: [1] } } }, COLUMNS))).toEqual({
      column: 'year',
      op: 'in',
      values: [1],
    });
  });
});

describe('the inclusive evaluator: a tile is dropped only when no row can match', () => {
  it('prunes each comparison at the bound, inclusively', () => {
    expect(on({ column: 'dense_id', op: '<', value: 64 })).toBe(false);
    expect(on({ column: 'dense_id', op: '<=', value: 64 })).not.toBe(false);
    expect(on({ column: 'dense_id', op: '>', value: 127 })).toBe(false);
    expect(on({ column: 'dense_id', op: '>=', value: 127 })).not.toBe(false);
    expect(on({ column: 'dense_id', op: '=', value: 200 })).toBe(false);
    expect(on({ column: 'dense_id', op: '=', value: 100 })).not.toBe(false);
  });

  it('keeps != and not in, which bounds cannot rule out', () => {
    expect(on({ column: 'dense_id', op: '!=', value: 100 })).not.toBe(false);
    expect(on({ column: 'dense_id', op: 'not in', values: [64, 65] })).not.toBe(false);
  });

  it('prunes in when no literal falls inside the bounds, and an empty in always', () => {
    expect(on({ column: 'year', op: 'in', values: [1900, 2000] })).toBe(false);
    expect(on({ column: 'year', op: 'in', values: [1900, 1970] })).not.toBe(false);
    expect(on({ column: 'year', op: 'in', values: [] })).toBe(false);
  });

  it('reads the null counts: is null, not null, and a column of nulls only', () => {
    expect(on({ column: 'dense_id', op: 'is null' })).toBe(false);
    expect(on({ column: 'year', op: 'is null' })).not.toBe(false);
    const nulls: Metrics = { ...TILE, nullValueCounts: { ...TILE.nullValueCounts, year: 64 } };
    expect(on({ column: 'year', op: 'not null' }, nulls)).toBe(false);
    expect(on({ column: 'year', op: '=', value: 1970 }, nulls)).toBe(false);
    expect(on({ column: 'year', op: '!=', value: 1970 }, nulls)).toBe(false);
    expect(on({ column: 'year', op: 'is null' }, nulls)).toBe(true);
  });

  it('keeps a tile with no statistics for the column, and a string always', () => {
    const bare: Metrics = { recordCount: 64, nullValueCounts: {}, lowerBounds: {}, upperBounds: {} };
    expect(on({ column: 'dense_id', op: '<', value: 0 }, bare)).not.toBe(false);
    expect(on({ column: 'subject', op: '=', value: 'nobody' })).not.toBe(false);
  });

  it('compares a float bound exactly, and a bigint literal against a number bound', () => {
    expect(on({ column: 'x', op: '>', value: 2.5 })).toBe(false);
    expect(on({ column: 'x', op: '>', value: 2.4999999 })).not.toBe(false);
    expect(on({ column: 'dense_id', op: '<', value: 64n })).toBe(false);
    expect(on({ column: 'dense_id', op: '>', value: 2n ** 60n })).toBe(false);
  });

  it('combines through and and or', () => {
    expect(on({ and: [{ column: 'dense_id', op: '<', value: 100 }, { column: 'year', op: '>', value: 2000 }] })).toBe(false);
    expect(on({ or: [{ column: 'dense_id', op: '<', value: 0 }, { column: 'year', op: '>', value: 2000 }] })).toBe(false);
    expect(on({ or: [{ column: 'dense_id', op: '<', value: 0 }, { column: 'year', op: '>', value: 1980 }] })).toEqual(
      bind({ column: 'year', op: '>', value: 1980 }, COLUMNS),
    );
  });
});

describe('the strict evaluator: a residual is null only where every row matches', () => {
  it('settles an integer range the bounds contain', () => {
    expect(on({ column: 'dense_id', op: '>=', value: 64 })).toBe(true);
    expect(on({ column: 'dense_id', op: '<', value: 128 })).toBe(true);
    expect(on({ and: [{ column: 'dense_id', op: '>=', value: 0 }, { column: 'dense_id', op: '<', value: 1000 }] })).toBe(true);
    expect(on({ column: 'dense_id', op: '!=', value: 5 })).toBe(true);
    expect(on({ column: 'dense_id', op: 'not in', values: [1, 500] })).toBe(true);
  });

  it('does not settle a column with nulls, whose nulls a comparison does not select', () => {
    expect(on({ column: 'year', op: '>=', value: 1900 })).not.toBe(true);
    expect(on({ column: 'year', op: 'not null' })).not.toBe(true);
    expect(on({ column: 'dense_id', op: 'not null' })).toBe(true);
  });

  it('never settles a float comparison, because the manifest counts no NaN', () => {
    expect(on({ column: 'x', op: '<', value: 10 })).not.toBe(true);
    expect(on({ column: 'x', op: '<', value: 10 })).not.toBe(false);
    expect(on({ column: 'x', op: '!=', value: 10 })).toBe(true);
  });

  it('folds settled leaves out of what is left', () => {
    const left = on({
      and: [
        { column: 'dense_id', op: '>=', value: 0 },
        { column: 'year', op: '>', value: 1960 },
      ],
    });
    expect(filterOf(left)).toEqual({ column: 'year', op: '>', value: 1960 });
    expect(on({ or: [{ column: 'dense_id', op: '>=', value: 0 }, { column: 'year', op: '>', value: 1960 }] })).toBe(true);
  });
});

describe('the residual as SQL', () => {
  const sql = (filter: Filter) => sqlOf(bind(filter, COLUMNS));

  it('compares a float as a double and keeps NaN out of > and >=, which DuckDB orders above every number', () => {
    expect(sql({ column: 'x', op: '<', value: 0.1 })).toBe('"x"::DOUBLE < 0.1::DOUBLE');
    expect(sql({ column: 'x', op: '>=', value: 1 })).toBe('("x"::DOUBLE >= 1::DOUBLE AND NOT isnan("x"))');
    expect(sql({ column: 'dense_id', op: '>', value: 1n })).toBe('"dense_id" > 1');
  });

  it('quotes identifiers and strings, and spells every operator', () => {
    expect(sql({ column: 'subject', op: '=', value: "o'hara" })).toBe(`"subject" = 'o''hara'`);
    expect(sql({ column: 'year', op: 'not in', values: [1, 2] })).toBe('"year" NOT IN (1, 2)');
    expect(sql({ column: 'year', op: 'in', values: [] })).toBe('FALSE');
    expect(sql({ not: { column: 'year', op: 'is null' } })).toBe('"year" IS NOT NULL');
    expect(sql({ or: [{ column: 'year', op: '!=', value: 3 }, { column: 'dense_id', op: '<=', value: 9 }] })).toBe(
      '("year" <> 3 OR "dense_id" <= 9)',
    );
  });
});
