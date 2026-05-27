/**
 * PLAY-05 example — R2RML-classic MusicBrainz subset (Artists).
 *
 * 5-artist sample → ex:Artist resources, typed via the bundled ShEx shape.
 *
 * Phase 15 plan 15-02 (BUG-02): dropped the `musicbrainz.csvw.json` sidecar
 * — post Phase 13 ADR-0037 the CLI infers the schema via DuckDB DESCRIBE;
 * the sidecar was dead weight. The ShEx sibling was converted from ShExC
 * compact syntax to ShEx 2.1 JSON-LD so `fossil-cli`'s sibling
 * auto-discovery (`ShExDescriptor::from_reader`, JSON-LD only) succeeds.
 */

import type { Example } from '../index.js';
import mapping from './musicbrainz.fossil?raw';
import csv from './musicbrainz-artists.csv?raw';
import shex from './musicbrainz.shex?raw';

export const musicbrainzExample: Example = {
  id: 'musicbrainz',
  title: 'MusicBrainz (R2RML-classic)',
  description:
    'Sample 5-artist MusicBrainz subset → ex:Artist resources typed via ShEx.',
  mapping,
  shex,
  dataFiles: [
    { path: 'musicbrainz-artists.csv', contents: csv, format: 'csv' },
  ],
};
