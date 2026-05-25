/**
 * Round-trip tests for the Turtle serializer (PLAY-10 foundation).
 *
 * The strongest proof that the output is valid Turtle is that n3.Parser
 * accepts it and emits the same quads we wrote. We round-trip every shape
 * we care about: empty, vertices-only, edges-only, mixed datatypes, null
 * skipping, escape-heavy strings.
 *
 * Per 09-04-PLAN Task 2 + 09-RESEARCH.md Pattern 4.
 */

import { describe, test, expect } from 'vitest';
import { Parser } from 'n3';
import {
  rowsToTurtle,
  type VertexRow,
  type EdgeRow,
} from '../src/turtle/index.js';

const prefixes = {
  ex: 'https://example.org/',
  rdf: 'http://www.w3.org/1999/02/22-rdf-syntax-ns#',
  xsd: 'http://www.w3.org/2001/XMLSchema#',
};

describe('rowsToTurtle (PLAY-10)', () => {
  test('empty input produces empty Turtle (or only prefix block)', () => {
    const ttl = rowsToTurtle([], [], prefixes);
    const parsed = new Parser().parse(ttl);
    expect(parsed).toHaveLength(0);
  });

  test('single vertex with type + 2 string props roundtrips through n3.Parser', () => {
    const vertices: VertexRow[] = [
      {
        iri: 'https://example.org/user/1',
        type: 'https://example.org/Person',
        props: {
          'https://example.org/name': 'Alice',
          'https://example.org/email': 'alice@example.org',
        },
      },
    ];
    const ttl = rowsToTurtle(vertices, [], prefixes);
    const quads = new Parser().parse(ttl);
    // 3 quads: rdf:type + name + email
    expect(quads).toHaveLength(3);
    const subjects = new Set(quads.map((q) => q.subject.value));
    expect(subjects).toEqual(new Set(['https://example.org/user/1']));
  });

  test('numeric props get xsd:integer / xsd:double datatypes', () => {
    const vertices: VertexRow[] = [
      {
        iri: 'https://example.org/p/1',
        type: 'https://example.org/P',
        props: {
          'https://example.org/age': 42,
          'https://example.org/score': 3.14,
        },
      },
    ];
    const ttl = rowsToTurtle(vertices, [], prefixes);
    const quads = new Parser().parse(ttl);
    const ageQ = quads.find(
      (q) => q.predicate.value === 'https://example.org/age',
    );
    const scoreQ = quads.find(
      (q) => q.predicate.value === 'https://example.org/score',
    );
    expect(ageQ?.object.termType).toBe('Literal');
    const ageDt =
      (ageQ?.object as { datatypeString?: string }).datatypeString ??
      (ageQ?.object as { datatype?: { value?: string } }).datatype?.value;
    const scoreDt =
      (scoreQ?.object as { datatypeString?: string }).datatypeString ??
      (scoreQ?.object as { datatype?: { value?: string } }).datatype?.value;
    expect(ageDt).toContain('integer');
    expect(scoreDt).toContain('double');
  });

  test('edges produce 1 quad each', () => {
    const edges: EdgeRow[] = [
      {
        src: 'https://example.org/a',
        pred: 'https://example.org/knows',
        dst: 'https://example.org/b',
      },
      {
        src: 'https://example.org/b',
        pred: 'https://example.org/knows',
        dst: 'https://example.org/a',
      },
    ];
    const ttl = rowsToTurtle([], edges, prefixes);
    const quads = new Parser().parse(ttl);
    expect(quads).toHaveLength(2);
    expect(
      quads.every((q) => q.predicate.value === 'https://example.org/knows'),
    ).toBe(true);
  });

  test('null prop values are skipped', () => {
    const vertices: VertexRow[] = [
      {
        iri: 'https://example.org/u/1',
        type: 'https://example.org/U',
        props: {
          'https://example.org/optional': null,
          'https://example.org/name': 'X',
        },
      },
    ];
    const ttl = rowsToTurtle(vertices, [], prefixes);
    const quads = new Parser().parse(ttl);
    // 1 rdf:type + 1 name (null skipped)
    expect(quads).toHaveLength(2);
  });

  test('strings containing newlines + quotes are properly escaped (no parse error)', () => {
    const vertices: VertexRow[] = [
      {
        iri: 'https://example.org/x',
        type: 'https://example.org/T',
        props: {
          'https://example.org/desc': 'line1\nline2 "quoted"',
        },
      },
    ];
    const ttl = rowsToTurtle(vertices, [], prefixes);
    expect(() => new Parser().parse(ttl)).not.toThrow();
    // And the escaped string round-trips bit-exact through n3's lexer:
    const quads = new Parser().parse(ttl);
    const descQ = quads.find(
      (q) => q.predicate.value === 'https://example.org/desc',
    );
    expect(descQ?.object.value).toBe('line1\nline2 "quoted"');
  });

  test('boolean props get xsd:boolean datatype', () => {
    const vertices: VertexRow[] = [
      {
        iri: 'https://example.org/u/1',
        type: 'https://example.org/U',
        props: {
          'https://example.org/active': true,
          'https://example.org/banned': false,
        },
      },
    ];
    const ttl = rowsToTurtle(vertices, [], prefixes);
    const quads = new Parser().parse(ttl);
    const activeQ = quads.find(
      (q) => q.predicate.value === 'https://example.org/active',
    );
    const bannedQ = quads.find(
      (q) => q.predicate.value === 'https://example.org/banned',
    );
    const activeDt =
      (activeQ?.object as { datatypeString?: string }).datatypeString ??
      (activeQ?.object as { datatype?: { value?: string } }).datatype?.value;
    expect(activeDt).toContain('boolean');
    expect(activeQ?.object.value).toBe('true');
    expect(bannedQ?.object.value).toBe('false');
  });
});
