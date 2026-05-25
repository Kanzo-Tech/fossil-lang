/**
 * PLAY-05 example — R2RML-classic MusicBrainz subset (Artists).
 *
 * 5-artist sample → ex:Artist resources, typed via the bundled ShEx shape.
 */

import type { Example } from '../index.js';
import mapping from './musicbrainz.fossil?raw';
import csv from './musicbrainz-artists.csv?raw';
import csvw from './musicbrainz.csvw.json?raw';
import shex from './musicbrainz.shex?raw';

export const musicbrainzExample: Example = {
  id: 'musicbrainz',
  title: 'MusicBrainz (R2RML-classic)',
  description:
    'Sample 5-artist MusicBrainz subset → ex:Artist resources typed via ShEx.',
  mapping,
  csvw,
  shex,
  dataFiles: [
    { path: 'musicbrainz-artists.csv', contents: csv, format: 'csv' },
  ],
};
