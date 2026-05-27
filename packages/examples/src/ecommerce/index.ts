/**
 * PLAY-05 example — R2RML-classic Northwind-subset (Customers).
 *
 * One row → one `ex:Customer` resource. The canonical relational-mapping
 * shape; the FK-join variant lives in `multi-source-join/`.
 *
 * Phase 15 plan 15-02 (BUG-02): dropped the `ecommerce.csvw.json` sidecar —
 * post Phase 13 ADR-0037 the CLI infers the schema via DuckDB DESCRIBE; the
 * sidecar was dead weight. The ShEx sibling was converted from ShExC compact
 * syntax to ShEx 2.1 JSON-LD so `fossil-cli`'s sibling auto-discovery
 * (`ShExDescriptor::from_reader`, JSON-LD only) succeeds.
 */

import type { Example } from '../index.js';
import mapping from './ecommerce.fossil?raw';
import csv from './ecommerce-customers.csv?raw';
import shex from './ecommerce.shex?raw';

export const ecommerceExample: Example = {
  id: 'ecommerce',
  title: 'E-commerce (R2RML-classic)',
  description:
    'Northwind-subset Customers table → ex:Customer resources with typed properties.',
  mapping,
  shex,
  dataFiles: [
    { path: 'ecommerce-customers.csv', contents: csv, format: 'csv' },
  ],
};
