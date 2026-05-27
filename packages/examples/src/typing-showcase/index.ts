/**
 * PLAY-05 example — Fossil-specific typing showcase.
 *
 * Highlights the `|>` pipeline + implicit closure synthesis (CORE-07,
 * type-system.md §7). The `filter(.age >= 18)` clause synthesises a closure
 * over the row binding without surface `\row -> ...` syntax.
 *
 * Phase 15 plan 15-02 (BUG-02): dropped the `typing-showcase.csvw.json`
 * sidecar — post Phase 13 ADR-0037 the CLI infers the schema via DuckDB
 * DESCRIBE; the sidecar was dead weight (the example already compiled
 * without it).
 */

import type { Example } from '../index.js';
import mapping from './typing-showcase.fossil?raw';
import csv from './typing-showcase.csv?raw';

export const typingShowcaseExample: Example = {
  id: 'typing-showcase',
  title: 'Typing showcase',
  description:
    'Pipeline `|>` + implicit closure synthesis — Fossil-specific feature demo (CORE-07).',
  mapping,
  // intentionally NO shex — keep the example focused on the pipeline feature.
  dataFiles: [
    { path: 'typing-showcase.csv', contents: csv, format: 'csv' },
  ],
};
