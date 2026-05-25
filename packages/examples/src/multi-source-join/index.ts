/**
 * PLAY-05 example — two-source (FK-join) mapping.
 *
 * `customers` + `orders` CSV sources emit independent mappings into the
 * same target graph; the FK is realised at the IRI-template level
 * (`ex:customer/{customer_id}` shared between both mappings).
 */

import type { Example } from '../index.js';
import mapping from './multi-source-join.fossil?raw';
import customersCsv from './msj-customers.csv?raw';
import ordersCsv from './msj-orders.csv?raw';
import csvw from './multi-source-join.csvw.json?raw';

export const multiSourceJoinExample: Example = {
  id: 'multi-source-join',
  title: 'Multi-source join (FK)',
  description:
    'Two CSV sources (customers + orders) → one graph linked by foreign-key IRI templates.',
  mapping,
  csvw,
  dataFiles: [
    { path: 'msj-customers.csv', contents: customersCsv, format: 'csv' },
    { path: 'msj-orders.csv',    contents: ordersCsv,    format: 'csv' },
  ],
};
