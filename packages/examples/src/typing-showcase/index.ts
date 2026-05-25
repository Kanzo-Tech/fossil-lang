/**
 * PLAY-05 example — Fossil-specific typing showcase.
 *
 * Highlights the `|>` pipeline + implicit closure synthesis (CORE-07,
 * type-system.md §7). The `filter(.age >= 18)` clause synthesises a closure
 * over the row binding without surface `\row -> ...` syntax.
 */

import type { Example } from '../index.js';
import mapping from './typing-showcase.fossil?raw';
import csv from './typing-showcase.csv?raw';
import csvw from './typing-showcase.csvw.json?raw';

export const typingShowcaseExample: Example = {
  id: 'typing-showcase',
  title: 'Typing showcase',
  description:
    'Pipeline `|>` + implicit closure synthesis — Fossil-specific feature demo (CORE-07).',
  mapping,
  csvw,
  // intentionally NO shex — keep the example focused on the pipeline feature.
  dataFiles: [
    { path: 'typing-showcase.csv', contents: csv, format: 'csv' },
  ],
};
