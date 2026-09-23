import { describe, expect, it } from 'vitest';
import type { CorpusTypes, CorpusVertexType } from '@fossil-lang/corpus';

import { categoricalOf, channelsFor, encodingFor, readChannels } from '../src/index.js';

/** A payload as `Corpus.types` reports it — the writer's five columns plus two of the source's. */
const person: CorpusVertexType = {
  type: 'Person',
  count: 1_000_000n,
  identity: 'subject',
  fields: [
    { name: 'dense_id', type: 'UINTEGER' },
    { name: 'subject', type: 'VARCHAR' },
    { name: 'x', type: 'FLOAT' },
    { name: 'y', type: 'FLOAT' },
    { name: 'cluster_id', type: 'UINTEGER' },
    { name: 'birth_year', type: 'INTEGER' },
    { name: 'postcode', type: 'VARCHAR' },
  ],
} as unknown as CorpusVertexType;

const types = { vertices: [person], edges: [] } as unknown as CorpusTypes;

const manifest = (body: string) => `type: Person\nprefix: vertex/Person/\n${body}`;

describe('readChannels — the three states of the block', () => {
  it('no key is not declared', () => {
    expect(readChannels(manifest('properties: []\n'))).toBeUndefined();
  });

  it('an empty sequence is the writer declaring none', () => {
    expect(readChannels(manifest('channels: []\n'))).toEqual([]);
  });

  it('a list is the declaration, and the block ends at the next top-level key', () => {
    const text = manifest(
      'channels:\n' +
        '- name: community\n' +
        '  column: cluster_id\n' +
        '  scale: categorical\n' +
        '  domain: 40\n' +
        '- name: age\n' +
        '  column: birth_year\n' +
        '  scale: quantitative\n' +
        'version: 1\n',
    );
    expect(readChannels(text)).toEqual([
      { name: 'community', column: 'cluster_id', scale: 'categorical', domain: 40 },
      { name: 'age', column: 'birth_year', scale: 'quantitative' },
    ]);
  });

  it('a scale outside the closed set is dropped, and a block of nothing else reads as EMPTY', () => {
    const text = manifest('channels:\n- name: n\n  column: c\n  scale: ordinal\n');
    expect(readChannels(text)).toEqual([]);
  });
});

describe('channelsFor', () => {
  const declared = manifest('channels:\n- name: c\n  column: cluster_id\n  scale: categorical\n');
  const other = 'type: Company\nchannels: []\n';

  it('reads the document for the type asked for', () => {
    expect(channelsFor([other, declared], 'Person')).toHaveLength(1);
    expect(channelsFor([other, declared], 'Company')).toEqual([]);
  });

  it('is undefined when no document is that type — the same answer as declaring nothing', () => {
    expect(channelsFor([other, declared], 'Nowhere')).toBeUndefined();
  });
});

describe('categoricalOf', () => {
  it('derives the convention for a corpus that declares nothing', () => {
    expect(categoricalOf(person, undefined)).toBe('cluster_id');
  });

  it('a declared categorical outranks the convention', () => {
    const channels = [{ name: 'p', column: 'birth_year', scale: 'categorical' as const }];
    expect(categoricalOf(person, channels)).toBe('birth_year');
  });

  it('a block that declares no categorical means there is nothing to colour by', () => {
    expect(categoricalOf(person, [])).toBeNull();
    const quantitative = [{ name: 'a', column: 'birth_year', scale: 'quantitative' as const }];
    expect(categoricalOf(person, quantitative)).toBeNull();
  });

  it('a declaration the bytes do not carry is not drawable', () => {
    const missing = [{ name: 'x', column: 'not_on_disk', scale: 'categorical' as const }];
    expect(categoricalOf(person, missing)).toBeNull();
  });
});

describe('encodingFor', () => {
  it('derives colour, chart and projection from the bytes alone', () => {
    const encoding = encodingFor({ types });
    expect(encoding).not.toBeNull();
    expect(encoding!.fill).toBe('cluster_id');
    expect(encoding!.label).toBe('subject');
    expect(encoding!.brush).toBe('birth_year');
    expect(encoding!.columns).toEqual(['dense_id', 'cluster_id', 'birth_year']);
  });

  it('projects the declared quasi-identifiers this type actually carries', () => {
    const encoding = encodingFor({
      types,
      quasiIdentifiers: ['Person.postcode', 'Person.absent', 'Company.name'],
    });
    expect(encoding!.columns).toEqual(['dense_id', 'cluster_id', 'birth_year', 'postcode']);
  });

  it('a quantitative channel outranks a quasi-identifier for the chart', () => {
    const encoding = encodingFor({
      types,
      channels: [{ name: 'age', column: 'birth_year', scale: 'quantitative' }],
      quasiIdentifiers: ['Person.postcode'],
    });
    expect(encoding!.brush).toBe('birth_year');
  });

  it('the brush is in the projection even when it is not a quasi-identifier', () => {
    const encoding = encodingFor({
      types,
      channels: [{ name: 'age', column: 'birth_year', scale: 'quantitative' }],
    });
    expect(encoding!.columns).toContain('birth_year');
  });

  it('a corpus with no vertex type at all is null, not an exception', () => {
    expect(encodingFor({ types: { vertices: [], edges: [] } as unknown as CorpusTypes })).toBeNull();
  });
});
