/**
 * `openProgram` is the protocol both editor hosts used to write by hand: push the text before any
 * question, resolve the documents it names before a check, and answer in the shape `fossil()`
 * takes. These pin each half across the real wasm boundary.
 */
import { describe, it, expect, beforeAll, afterEach, vi } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { initFossilWasm, openProgram, type FossilProgram } from '../src/index.js';
import { recordingHost } from './host.js';

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

afterEach(() => vi.unstubAllGlobals());

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
    const { host, asked, fetched } = recordingHost(SHAPE);
    program = await openProgram('prog.fossil', { host, text: PROGRAM });
    try {
      expect(asked).toEqual([{ connection: 'vocab' }]);
      expect(fetched).toHaveLength(1);
      expect(fetched[0]).toBe('http://minio.example/vocab/shapes/person.shex');
      program.registerDescriptor(DESCRIPTOR);
      const rows = await program.check(PROGRAM);
      // The shape arrived: `name` is checked against its datatype, which only a read shape knows.
      expect(rows.some((r) => r.message.includes('expects Integer'))).toBe(true);
      expect(rows.every((r) => r.uri === 'prog.fossil')).toBe(true);
      // Nothing new is missing, so a second check reads nothing.
      await program.check(PROGRAM);
      expect(fetched).toHaveLength(1);
    } finally {
      program.close();
    }
  });

  it('answers a position query about the text it was handed, not the last check', async () => {
    const { host } = recordingHost(SHAPE);
    program = await openProgram('prog.fossil', { host });
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
    const { host } = recordingHost(SHAPE);
    program = await openProgram('prog.fossil', { host });
    try {
      expect(await program.sources(PROGRAM)).toEqual([
        { binding: 'users', key: '@lake/users.csv', locator: 's3://lake/users.csv', connection: 'lake', format: 'csv' },
      ]);
    } finally {
      program.close();
    }
  });

  it('carries exactly the option names fossil() takes', async () => {
    const { host } = recordingHost(SHAPE);
    program = await openProgram('prog.fossil', { host });
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

describe('openProgram, when a document cannot be read', () => {
  const MISSING = PROGRAM.replace('@vocab/person.shex', '@vocab/gone.shex');

  it('answers a row carrying the document’s problem, beside the checker’s rows', async () => {
    const { host } = recordingHost(SHAPE);
    const program = await openProgram('prog.fossil', { host, text: MISSING });
    try {
      const rows = await program.check(MISSING);
      expect(rows[0]).toMatchObject({
        uri: 'prog.fossil',
        problem: { code: 'storage/unreachable', data: { locator: expect.stringContaining('gone.shex') } },
      });
    } finally {
      program.close();
    }
  });

  it('answers storage/host-silent once the host has been quiet for 30 s, instead of never painting', async () => {
    const { host } = recordingHost(SHAPE);
    const program = await openProgram('prog.fossil', { host, text: MISSING });
    vi.useFakeTimers();
    try {
      host.connections = () => new Promise(() => {});
      const outcome = program.check(MISSING).catch((e: unknown) => e);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(await outcome).toMatchObject({ code: 'storage/host-silent' });
    } finally {
      vi.useRealTimers();
      program.close();
    }
  });
});
