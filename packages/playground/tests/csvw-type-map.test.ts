/**
 * Tests for duckdbTypeToCsvw — pure DuckDB → CSVW XSD-short-name map.
 *
 * Foundation for PLAY-09 + PLAY-11 (CSVW preview + inference). The map is
 * tested in isolation (no DuckDB) so any divergence between DuckDB's
 * DESCRIBE output and the CSVW W3C Rec datatype list shows up here first.
 *
 * Per 09-04-PLAN Task 3 + 09-RESEARCH.md Pattern 2.
 */

import { describe, test, expect } from 'vitest';
import { duckdbTypeToCsvw } from '../src/csvw/index.js';

describe('duckdbTypeToCsvw (PLAY-09/PLAY-11)', () => {
  test.each([
    // Booleans
    ['BOOLEAN', 'boolean'],
    // Signed integers
    ['TINYINT', 'integer'],
    ['SMALLINT', 'integer'],
    ['INTEGER', 'integer'],
    ['BIGINT', 'integer'],
    ['HUGEINT', 'integer'],
    // Unsigned integers
    ['UTINYINT', 'nonNegativeInteger'],
    ['USMALLINT', 'nonNegativeInteger'],
    ['UINTEGER', 'nonNegativeInteger'],
    ['UBIGINT', 'nonNegativeInteger'],
    // Floats
    ['FLOAT', 'float'],
    ['REAL', 'float'],
    ['DOUBLE', 'double'],
    // Fixed-point
    ['DECIMAL', 'decimal'],
    ['DECIMAL(10,2)', 'decimal'],
    ['NUMERIC', 'decimal'],
    // Temporal
    ['DATE', 'date'],
    ['TIME', 'time'],
    ['TIMESTAMP', 'dateTime'],
    ['DATETIME', 'dateTime'],
    ['TIMESTAMP WITH TIME ZONE', 'dateTimeStamp'],
    ['TIMESTAMPTZ', 'dateTimeStamp'],
    ['INTERVAL', 'duration'],
    // Binary / opaque
    ['BLOB', 'hexBinary'],
    ['BYTEA', 'hexBinary'],
    ['UUID', 'string'],
    // Strings
    ['VARCHAR', 'string'],
    ['TEXT', 'string'],
    ['STRING', 'string'],
    // Safe fallback
    ['UNKNOWN_TYPE', 'string'],
  ])('maps DuckDB %s → CSVW %s', (duckdb, expected) => {
    expect(duckdbTypeToCsvw(duckdb)).toBe(expected);
  });

  test('case-insensitive', () => {
    expect(duckdbTypeToCsvw('integer')).toBe('integer');
    expect(duckdbTypeToCsvw('BigInt')).toBe('integer');
    expect(duckdbTypeToCsvw('varchar')).toBe('string');
    expect(duckdbTypeToCsvw('timestamp')).toBe('dateTime');
  });

  test('handles parameterized DECIMAL with various precisions', () => {
    expect(duckdbTypeToCsvw('DECIMAL(18,4)')).toBe('decimal');
    expect(duckdbTypeToCsvw('DECIMAL(5,0)')).toBe('decimal');
    expect(duckdbTypeToCsvw('NUMERIC(10,2)')).toBe('decimal');
  });
});
