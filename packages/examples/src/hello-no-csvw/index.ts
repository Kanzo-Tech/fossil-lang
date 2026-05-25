/**
 * PLAY-05 example — hello variant WITHOUT a CSVW descriptor.
 *
 * Demonstrates the PLAY-11 inference path: paste raw CSV with no descriptor
 * → DuckDB `read_csv_auto` infers column types → playground populates the
 * CSVW panel automatically. Distinct from the canonical `hello` example in
 * that `csvw` is intentionally `undefined` here.
 *
 * Mirrors the hello/ structure (raw `?raw` imports + Example record).
 */

import type { Example } from '../index.js';
import mapping from './hello-no-csvw.fossil?raw';
import csv from './hello-no-csvw.csv?raw';

export const helloNoCsvwExample: Example = {
  id: 'hello-no-csvw',
  title: 'Hello (no CSVW — inference)',
  description:
    'Same 5-user CSV as hello, but no descriptor — exercises PLAY-11 inference (DuckDB read_csv_auto).',
  mapping,
  // intentionally NO csvw — that's the whole point.
  // intentionally NO shex — keep the variant focused on inference.
  dataFiles: [{ path: 'hello-no-csvw.csv', contents: csv, format: 'csv' }],
};
