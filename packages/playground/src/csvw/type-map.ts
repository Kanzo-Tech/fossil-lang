/**
 * DuckDB → CSVW XSD-short-name type map.
 *
 * Pure function — no DuckDB dep. Used by `inferCsvw()` and is also unit-tested
 * in isolation so the table stays in sync with both DuckDB's `DESCRIBE` output
 * and the CSVW W3C Rec datatype list.
 *
 * Per 09-RESEARCH.md Pattern 2 + 09-CONTEXT.md decisions (CSVW inference).
 * Foundation for PLAY-09 + PLAY-11.
 */

/**
 * Map a DuckDB type string (as returned by `DESCRIBE SELECT * ...`) to a CSVW
 * datatype XSD short name (e.g., `"integer"`, `"string"`, `"dateTime"`).
 *
 * Strips parameter suffixes (`DECIMAL(10,2)` → `DECIMAL`) and is case-insensitive.
 * Falls back to `"string"` for any unrecognized type — never throws.
 *
 * @param duckdbType  DuckDB type string (e.g., `"INTEGER"`, `"VARCHAR"`, `"DECIMAL(10,2)"`).
 * @returns           CSVW XSD short name (e.g., `"integer"`, `"string"`, `"decimal"`).
 */
export function duckdbTypeToCsvw(duckdbType: string): string {
  // Strip parameter suffix and normalize case.
  const base = duckdbType.replace(/\(.*\)/, '').toUpperCase().trim();

  switch (base) {
    case 'BOOLEAN':
      return 'boolean';

    case 'TINYINT':
    case 'SMALLINT':
    case 'INTEGER':
    case 'BIGINT':
    case 'HUGEINT':
      return 'integer';

    case 'UTINYINT':
    case 'USMALLINT':
    case 'UINTEGER':
    case 'UBIGINT':
      return 'nonNegativeInteger';

    case 'FLOAT':
    case 'REAL':
      return 'float';

    case 'DOUBLE':
      return 'double';

    case 'DECIMAL':
    case 'NUMERIC':
      return 'decimal';

    case 'DATE':
      return 'date';

    case 'TIME':
      return 'time';

    case 'TIMESTAMP':
    case 'DATETIME':
      return 'dateTime';

    case 'TIMESTAMP WITH TIME ZONE':
    case 'TIMESTAMPTZ':
      return 'dateTimeStamp';

    case 'INTERVAL':
      return 'duration';

    case 'BLOB':
    case 'BYTEA':
      return 'hexBinary';

    case 'UUID':
      return 'string';

    case 'VARCHAR':
    case 'TEXT':
    case 'STRING':
      return 'string';

    default:
      // Safe fallback — CSVW treats `string` as the universal default.
      return 'string';
  }
}
