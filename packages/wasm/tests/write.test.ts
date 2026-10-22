/**
 * `write` across the wasm boundary. The rules are `fossil-ide`'s (`tests/model.rs`); this holds
 * that a `Model` reaches them in its wire shape, and that what comes back is a program the checker
 * opens.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import type { Model } from '@fossil-lang/types';
import { initFossilWasm, openProgram, write } from '../src/index.js';
import { recordingHost } from './host.js';

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
});

const MODEL: Model = {
  sources: [
    { name: 'laureates', format: 'csv', reference: 'laureates.csv' },
    { name: 'institutions', format: 'csv', reference: 'institutions.csv' },
    { name: 'affiliations', format: 'csv', reference: 'affiliations.csv', option: '|' },
  ],
  types: [
    {
      name: 'Laureates',
      source: 'laureates',
      identity: ['id'],
      fields: [
        { name: 'name', column: 'name', primitive: 'string' },
        { name: 'born', column: 'born', primitive: 'time_date', optional: true },
      ],
    },
    { name: 'Institutions', source: 'institutions', identity: ['id'], fields: [{ name: 'title', column: 'title', primitive: 'string' }] },
  ],
  edges: [
    { name: 'affiliation', from: 'Laureates', to: 'Institutions', on: 'affiliations', column: 'institution', join: 'laureate', many: true },
  ],
};

describe('write', () => {
  it('declares the model under the base and maps it, the link file joined', async () => {
    const program = await write(MODEL, { base: 'https://acme.example' });
    expect(program).toContain('@iri("https://acme.example/vocab/Laureates")\ntype Laureates\n');
    expect(program).toContain('    @iri("https://acme.example/vocab/affiliation") affiliation: Seq<Ref<Institutions>>\n');
    expect(program).toContain('affiliations := io.csv("affiliations.csv", delimiter = "|")\n');
    expect(program).toContain(
      'LaureatesAffiliation : Laureates from laureates.join(affiliations, on = laureates.id == affiliations.laureate)\n',
    );
  });

  it('is a program the checker parses', async () => {
    const text = await write(MODEL, { base: 'https://acme.example' });
    const { host } = recordingHost('{}');
    const program = await openProgram('written.fossil', { host, text });
    try {
      const syntax = (await program.diagnostics()).filter((d) => d.code.startsWith('syntax/'));
      expect(syntax).toEqual([]);
    } finally {
      program.close();
    }
  });
});
