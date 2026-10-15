/**
 * `vocabularyOf` and `subjectOf` across the wasm boundary. The rules are `fossil-ide`'s
 * (`tests/constrain.rs`); this holds that they reach JS in the wire shapes, and that what
 * `vocabularyOf` answers is what `grammarFor` takes and a program accepts.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import type { Model, SourceVocabulary } from '@fossil-lang/types';
import { grammarFor, initFossilWasm, openProgram, subjectOf, vocabularyOf } from '../src/index.js';
import { recordingHost } from './host.js';

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
});

const BASE = 'https://acme.example';

const MODEL: Model = {
  types: [
    {
      name: 'Laureate',
      source: 'laureates',
      identity: ['id'],
      fields: [
        { name: 'name', primitive: 'string' },
        { name: 'born', primitive: 'date', optional: true },
        { name: 'since', primitive: 'g_year' },
      ],
    },
    { name: 'Institution', source: 'institutions', identity: ['id'], fields: [{ name: 'title', primitive: 'string' }] },
  ],
  edges: [{ name: 'affiliation', from: 'Laureate', to: 'Institution', on: 'affiliations', many: true }],
};

const csv = (name: string, columns: string[], delimiter?: string): SourceVocabulary => ({
  name,
  format: 'csv',
  reference: `${name}.csv`,
  columns,
  ...(delimiter ? { delimiter } : {}),
});

const SOURCES = [
  csv('laureates', ['id', 'name', 'born', 'since']),
  csv('institutions', ['id', 'title']),
  csv('affiliations', ['laureate', 'institution'], '|'),
  csv('countries', ['code']),
];

describe('vocabularyOf', () => {
  it('declares the model under the base, with the checker’s own spelling of each type', async () => {
    const { declared, sources } = await vocabularyOf(MODEL, SOURCES, BASE);
    const laureate = declared!.find((t) => t.name === 'Laureate')!;
    expect(laureate.iri).toBe('https://acme.example/vocab/Laureate');
    expect(laureate.fields).toEqual([
      { name: 'name', iri: 'https://acme.example/vocab/name', type: 'String' },
      { name: 'born', iri: 'https://acme.example/vocab/born', type: 'Option<Date>' },
      { name: 'since', iri: 'https://acme.example/vocab/since', type: 'GYear' },
      { name: 'affiliation', iri: 'https://acme.example/vocab/affiliation', type: 'Seq<Ref<Institution>>' },
    ]);
    expect(sources.map((s) => s.name)).toEqual(['laureates', 'institutions', 'affiliations']);
  });

  it('is a vocabulary the grammar takes and the checker accepts as a program', async () => {
    const vocabulary = await vocabularyOf(MODEL, SOURCES, BASE);
    const { prelude, grammar } = await grammarFor(vocabulary);
    expect(grammar).toContain('"laureates.name"');
    const { host } = recordingHost('{}');
    const program = await openProgram('prelude.fossil', { host, text: prelude });
    try {
      const errors = (await program.diagnostics()).filter((d) => d.severity === 1);
      expect(errors).toEqual([]);
    } finally {
      program.close();
    }
  });
});

describe('subjectOf', () => {
  it('is the workspace’s prefix, the type in words, and the identity as holes', async () => {
    expect(await subjectOf(MODEL.types[0]!, BASE)).toBe('https://acme.example/id/laureate/{laureates.id}');
    const link = { name: 'LaureateAward', source: 'awards', identity: ['laureate', 'prize'], fields: [] };
    expect(await subjectOf(link, `${BASE}/`)).toBe('https://acme.example/id/laureate-award/{awards.laureate}-{awards.prize}');
  });
});
