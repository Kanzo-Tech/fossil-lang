/**
 * `openProgram` is the protocol both editor hosts used to write by hand: push the text before any
 * question, resolve the documents it names before a check, and answer in the shape `fossil()`
 * takes. These pin each half across the real wasm boundary.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import type { SourceHost } from '@fossil-lang/types';
import { initFossilWasm, openProgram, type FossilProgram } from '../src/index.js';

beforeAll(async () => {
  const wasmPath = fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url));
  await initFossilWasm(await readFile(wasmPath));
});

const SHAPE = JSON.stringify({
  '@context': 'http://www.w3.org/ns/shex.jsonld',
  type: 'Schema',
  shapes: [
    {
      type: 'ShapeDecl',
      id: 'http://example.org/Person',
      shapeExpr: {
        type: 'Shape',
        expression: {
          type: 'TripleConstraint',
          predicate: 'http://example.org/name',
          valueExpr: { type: 'NodeConstraint', datatype: 'http://www.w3.org/2001/XMLSchema#integer' },
        },
      },
    },
  ],
});

const PROGRAM = `type { Person } := io.shex("@vocab/person.shex")
users := io.csv("@lake/users.csv")
User : Person from users
    @subject = "http://example.org/u/{users.id}"
    name = users.name
`;

const CONNECTIONS = { vocab: 'https://minio.example/shapes', lake: 's3://lake' };

function recordingHost() {
  const signed: string[] = [];
  const host: SourceHost = {
    connections: async () => CONNECTIONS,
    sign: async (locators) => {
      signed.push(...locators);
      return Object.fromEntries(locators.map((l) => [l, `${l}?signed`]));
    },
  };
  const fetchImpl = (async (url: string) =>
    url === 'https://minio.example/shapes/person.shex?signed'
      ? new Response(SHAPE)
      : new Response('', { status: 404 })) as typeof fetch;
  return { host, fetchImpl, signed };
}

const DESCRIPTOR = {
  uri: '@lake/users.csv',
  columns: [
    { name: 'id', primitive: 'string' as const },
    { name: 'name', primitive: 'string' as const },
  ],
  freshness_token: '',
};

describe('openProgram', () => {
  let program: FossilProgram | undefined;

  it('reads the documents the text names at open, through the host, once', async () => {
    const { host, fetchImpl, signed } = recordingHost();
    program = await openProgram('prog.fossil', { host, text: PROGRAM, fetch: fetchImpl });
    try {
      expect(signed).toEqual(['https://minio.example/shapes/person.shex']);
      program.registerDescriptor(DESCRIPTOR);
      const rows = await program.check(PROGRAM);
      // The shape arrived: `name` is checked against its datatype, which only a read shape knows.
      expect(rows.some((r) => r.message.includes('expects Integer'))).toBe(true);
      expect(rows.every((r) => r.uri === 'prog.fossil')).toBe(true);
      // Nothing new is missing, so a second check signs nothing.
      await program.check(PROGRAM);
      expect(signed).toHaveLength(1);
    } finally {
      program.close();
    }
  });

  it('answers a position query about the text it was handed, not the last check', async () => {
    const { host, fetchImpl } = recordingHost();
    program = await openProgram('prog.fossil', { host, fetch: fetchImpl });
    try {
      program.registerDescriptor(DESCRIPTOR);
      // Opened empty and never checked: hover must push the text itself to find anything.
      const line = PROGRAM.split('\n').findIndex((l) => l.includes('name = users.name'));
      const character = PROGRAM.split('\n')[line]!.indexOf('users.name') + 'users.'.length + 1;
      expect(program.hover(PROGRAM, line, character)).not.toBeNull();
      expect(program.complete(PROGRAM, line, character).length).toBeGreaterThan(0);
    } finally {
      program.close();
    }
  });

  it('reports the sources through the connection map', async () => {
    const { host, fetchImpl } = recordingHost();
    program = await openProgram('prog.fossil', { host, fetch: fetchImpl });
    try {
      expect(await program.sources(PROGRAM)).toEqual([
        { binding: 'users', key: '@lake/users.csv', locator: 's3://lake/users.csv', format: 'csv' },
      ]);
    } finally {
      program.close();
    }
  });

  it('carries exactly the option names fossil() takes', async () => {
    const { host, fetchImpl } = recordingHost();
    program = await openProgram('prog.fossil', { host, fetch: fetchImpl });
    try {
      for (const key of ['uri', 'tokenize', 'tokenKinds', 'check', 'hover', 'complete', 'definition']) {
        expect(program, key).toHaveProperty(key);
      }
      expect(program.tokenize('x := 1').length).toBeGreaterThan(0);
    } finally {
      program.close();
    }
  });
});
