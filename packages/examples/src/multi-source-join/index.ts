/**
 * PLAY-05 example — two-source (FK-join) mapping.
 *
 * `customers` + `orders` CSV sources emit independent mappings into the
 * same target graph; the FK is realised at the IRI-template level
 * (`ex:customer/{customer_id}` shared between both mappings).
 *
 * Phase 15 plan 15-02 (BUG-02): dropped the `multi-source-join.csvw.json`
 * sidecar — post Phase 13 ADR-0037 the CLI infers the schema via DuckDB
 * DESCRIBE; the sidecar was dead weight (the example already compiled
 * without it).
 */

import type { Example } from '../index.js';
import mapping from './multi-source-join.fossil?raw';
import customersCsv from './msj-customers.csv?raw';
import ordersCsv from './msj-orders.csv?raw';

export const multiSourceJoinExample: Example = {
  id: 'multi-source-join',
  title: 'Multi-source join (FK)',
  description:
    'Two CSV sources (customers + orders) → one graph linked by foreign-key IRI templates.',
  mapping,
  dataFiles: [
    { path: 'msj-customers.csv', contents: customersCsv, format: 'csv' },
    { path: 'msj-orders.csv',    contents: ordersCsv,    format: 'csv' },
  ],
};
