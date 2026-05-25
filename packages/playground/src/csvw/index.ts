/**
 * Barrel for the CSVW inference module.
 *
 * Public API (re-exported from `@fossil-lang/playground`):
 *   - inferCsvw(conn, csvUrl) — async DuckDB DESCRIBE → minimal CSVW JSON-LD
 *   - duckdbTypeToCsvw(type) — pure DuckDB → XSD-short-name mapping
 *   - CsvwTable, CsvwColumn, DescribingConnection — shapes
 *
 * Foundation for PLAY-09 + PLAY-11.
 */

export { inferCsvw } from './infer.js';
export type { CsvwTable, CsvwColumn, DescribingConnection } from './infer.js';
export { duckdbTypeToCsvw } from './type-map.js';
