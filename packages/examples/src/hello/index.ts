/**
 * The canonical "10 second" walking-skeleton example — mirrors the cargo
 * CLI's `examples/hello.fossil` byte-for-byte except for the source URI
 * rewrite (file path → `@examples/hello.csv` connector path per CONN-03 /
 * ADR-0029). A smoke test in `tests/examples.test.ts` pins the two copies'
 * prefix declarations to catch grammar drift.
 *
 * All four resource files are loaded as raw strings via Vite `?raw` imports;
 * the ambient module shim lives in `src/types.d.ts`. tsc accepts the imports
 * but does NOT copy the raw files — that's `scripts/copy-fixtures.mjs`'s job
 * (postbuild step).
 */

import type { Example } from '../index.js';
import helloMapping from './hello.fossil?raw';
import helloCsv from './hello.csv?raw';
import helloCsvw from './hello.csvw.json?raw';
import helloShex from './hello.shex?raw';

export const helloExample: Example = {
  id: 'hello',
  title: 'Hello, Fossil',
  description:
    'The canonical 10-second example — CSV → typed triples via implicit closure synthesis.',
  mapping: helloMapping,
  csvw: helloCsvw,
  shex: helloShex,
  dataFiles: [{ path: 'hello.csv', contents: helloCsv, format: 'csv' }],
};
