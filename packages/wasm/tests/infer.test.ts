/**
 * `infer` across the wasm boundary. The rules are `fossil-ide`'s (`tests/model.rs`, and
 * `fossil-df`'s `tests/written.rs` over real files); this holds that a `Profile` and the choices
 * reach them in their wire shape, and that the model comes back in its own.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import type { Profile, SourceProfile } from '@fossil-lang/types';
import { infer, initFossilWasm } from '../src/index.js';

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
});

function source(binding: string, columns: string[], keys: string[], option?: string): SourceProfile {
  return {
    binding,
    format: 'csv',
    option,
    descriptor: { key: `${binding}.csv`, columns: columns.map((name) => ({ name, primitive: 'string' })), etag: '' },
    rows: 3,
    sampled: false,
    columns: columns.map((name) => ({ name, null_count: 0, distinct_count: 3 })),
    candidate_keys: keys,
    sample: [],
  };
}

const PROFILE: Profile = {
  sources: [
    source('laureates', ['id', 'name'], ['id']),
    source('institutions', ['id', 'city'], ['id']),
    source('affiliations', ['laureate', 'institution'], [], '|'),
  ],
  inclusions: [
    { from: { source: 'affiliations', column: 'laureate' }, to: { source: 'laureates', column: 'id' } },
    { from: { source: 'affiliations', column: 'institution' }, to: { source: 'institutions', column: 'id' } },
  ],
  failed: [],
};

describe('infer', () => {
  it('reads a link file as a relation, and asks whether it is one', async () => {
    const { model, ambiguities } = await infer(PROFILE);
    expect(ambiguities).toEqual([
      {
        id: 'count/affiliations',
        about: { kind: 'count', source: 'affiliations', from: 'Laureates', to: 'Institutions' },
        options: ['link', 'thing'],
        chosen: 'link',
      },
    ]);
    expect(model.types.map((t) => t.name)).toEqual(['Laureates', 'Institutions']);
    expect(model.edges).toEqual([
      { name: 'institution', from: 'Laureates', to: 'Institutions', on: 'affiliations', column: 'institution', join: 'laureate', many: true, optional: false },
    ]);
    expect(model.sources[2]).toEqual({ name: 'affiliations', format: 'csv', reference: 'affiliations.csv', option: '|' });
  });

  it('takes a choice by its id', async () => {
    const { model } = await infer(PROFILE, { choices: { 'count/affiliations': 'thing' } });
    expect(model.types.map((t) => [t.name, t.identity])).toContainEqual(['Affiliations', ['laureate', 'institution']]);
  });
});
