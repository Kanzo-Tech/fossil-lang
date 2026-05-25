/**
 * PLAY-05 example — R2RML-classic Northwind-subset (Customers).
 *
 * One row → one `ex:Customer` resource. The canonical relational-mapping
 * shape; the FK-join variant lives in `multi-source-join/`.
 */

import type { Example } from '../index.js';
import mapping from './ecommerce.fossil?raw';
import csv from './ecommerce-customers.csv?raw';
import csvw from './ecommerce.csvw.json?raw';
import shex from './ecommerce.shex?raw';

export const ecommerceExample: Example = {
  id: 'ecommerce',
  title: 'E-commerce (R2RML-classic)',
  description:
    'Northwind-subset Customers table → ex:Customer resources with typed properties.',
  mapping,
  csvw,
  shex,
  dataFiles: [
    { path: 'ecommerce-customers.csv', contents: csv, format: 'csv' },
  ],
};
