/**
 * `open` is the protocol both editor hosts used to write by hand: the text arrives through
 * `update`, the documents it names are resolved before a check, and every question takes only a
 * position. These pin each half across the real wasm boundary.
 */
import { describe, it, expect, beforeAll, afterEach, vi } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { initFossilWasm, open, type FossilProgram } from '../src/index.js';
import { describingEngine, recordingHost } from './host.js';

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

/** `users.csv` as DuckDB describes it: both columns text. */
const USERS = describingEngine([
  { column_name: 'id', column_type: 'VARCHAR' },
  { column_name: 'name', column_type: 'VARCHAR' },
]);
const TYPES = { depth: 'types' } as const;

describe('open', () => {
  let program: FossilProgram | undefined;

  it('reads the documents the text names at open, through the host, once', async () => {
    const { host, asked, fetched } = recordingHost(SHAPE);
    program = await open('prog.fossil', { host, text: PROGRAM });
    try {
      expect(asked).toEqual([{ connection: 'vocab' }]);
      expect(fetched).toHaveLength(1);
      expect(fetched[0]).toBe('http://minio.example/vocab/shapes/person.shex');
      await program.describe({ engine: USERS, options: TYPES });
      const rows = await program.diagnostics();
      // The shape arrived: `name` is checked against its datatype, which only a read shape knows.
      const mismatch = rows.find((r) => r.code === 'type/property-mismatch');
      expect(mismatch?.code === 'type/property-mismatch' && mismatch.data.expected).toBe('Integer');
      expect(mismatch?.title).toBe('A property of the wrong type');
      expect(mismatch?.message).not.toContain('help:');
      expect(rows.every((r) => r.uri === 'prog.fossil')).toBe(true);
      // Nothing new is missing, so a second check reads nothing.
      await program.diagnostics();
      expect(fetched).toHaveLength(1);
    } finally {
      program.close();
    }
  });

  it('answers a position query about the text of the last update, with no check between', async () => {
    const { host } = recordingHost(SHAPE);
    program = await open('prog.fossil', { host });
    try {
      const line = PROGRAM.split('\n').findIndex((l) => l.includes('name = users.name'));
      const character = PROGRAM.split('\n')[line]!.indexOf('users.name') + 'users.'.length + 1;
      // Opened empty: nothing is under any cursor until the text arrives.
      expect(program.hover(line, character)).toBeNull();
      program.update(PROGRAM);
      // `name` as the shape types it, so the expression types and hover has a type to show.
      const typed = describingEngine([
        { column_name: 'id', column_type: 'VARCHAR' },
        { column_name: 'name', column_type: 'BIGINT' },
      ]);
      await program.describe({ engine: typed, options: TYPES });
      expect(program.hover(line, character)).not.toBeNull();
      expect(program.completion(line, character).length).toBeGreaterThan(0);
    } finally {
      program.close();
    }
  });

  it('names shapes, declarations and connections across the boundary', async () => {
    const { host } = recordingHost(SHAPE);
    program = await open('prog.fossil', { host });
    try {
      program.update(PROGRAM);
      const rows = program.semanticTokens();
      const lines = PROGRAM.split('\n');
      const at = (text: string) =>
        rows.find((r) => {
          const line = lines[r.range.start.line]!;
          return line.slice(r.range.start.character, r.range.end.character) === text;
        });
      expect(at('Person')).toMatchObject({ kind: 'type', modifiers: ['declaration'] });
      expect(at('users')).toMatchObject({ kind: 'variable', modifiers: ['declaration'] });
      expect(at('@lake')).toMatchObject({ kind: 'namespace', modifiers: [] });
      expect(at('io')).toMatchObject({ kind: 'namespace' });
      expect(at('csv')).toMatchObject({ kind: 'function' });
      expect(at('name')).toMatchObject({ kind: 'property' });
    } finally {
      program.close();
    }
  });

  it('reports its inputs through the connection map', async () => {
    const { host } = recordingHost(SHAPE);
    program = await open('prog.fossil', { host, text: PROGRAM });
    try {
      expect(await program.inputs()).toEqual([
        {
          role: 'data',
          binding: 'users',
          key: '@lake/users.csv',
          location: 's3://lake/users.csv',
          connection: 'lake',
          format: 'csv',
        },
        { role: 'schema', key: '@vocab/person.shex', location: 's3://vocab/shapes/person.shex', connection: 'vocab' },
      ]);
    } finally {
      program.close();
    }
  });

  it('describes what it reads through its own host, and answers the profile a run takes', async () => {
    const { host, asked } = recordingHost(SHAPE);
    program = await open('prog.fossil', { host, text: PROGRAM });
    try {
      const profiled = await program.describe({ engine: USERS, options: TYPES });
      expect(asked).toContainEqual({ connection: 'lake' });
      expect(profiled.descriptors).toEqual([
        {
          key: '@lake/users.csv',
          columns: [
            { name: 'id', primitive: 'string' },
            { name: 'name', primitive: 'string' },
          ],
          etag: '',
        },
      ]);
      expect(profiled.undescribed).toEqual([]);
      expect(profiled.profile.sources.map((s) => [s.binding, s.descriptor.key])).toEqual([['users', '@lake/users.csv']]);
    } finally {
      program.close();
    }
  });
});

describe('open, when a source cannot be described', () => {
  it('answers one warning at the call that reads it, under the code the profile answered', async () => {
    const { host } = recordingHost(SHAPE);
    const program = await open('prog.fossil', { host, text: PROGRAM });
    try {
      const unreadable = describingEngine([], new Error('CORS / unreachable'));
      const { undescribed } = await program.describe({ engine: unreadable, options: TYPES });
      expect(undescribed.map((u) => [u.source.key, u.problem.code])).toEqual([['@lake/users.csv', 'engine/failed']]);
      const rows = (await program.diagnostics()).filter((r) => r.code === 'engine/failed');
      expect(rows).toHaveLength(1);
      expect(rows[0]).toMatchObject({
        uri: 'prog.fossil',
        severity: 2,
        range: { start: { line: 1, character: 9 }, end: { line: 1, character: 34 } },
      });

      await program.describe({ engine: USERS, options: TYPES });
      expect((await program.diagnostics()).some((r) => r.code === 'engine/failed')).toBe(false);
    } finally {
      program.close();
    }
  });
});

describe('open, when a document cannot be read', () => {
  const MISSING = PROGRAM.replace('@vocab/person.shex', '@vocab/gone.shex');

  it('answers a row under the code of the document’s read, beside the checker’s rows', async () => {
    const { host } = recordingHost(SHAPE);
    const program = await open('prog.fossil', { host, text: MISSING });
    try {
      const rows = await program.diagnostics();
      expect(rows[0]).toMatchObject({
        uri: 'prog.fossil',
        code: 'storage/unreachable',
        data: { location: expect.stringContaining('gone.shex') },
      });
    } finally {
      program.close();
    }
  });

  it('answers storage/host-silent once the host has been quiet for 30 s, instead of never painting', async () => {
    const { host } = recordingHost(SHAPE);
    const program = await open('prog.fossil', { host, text: MISSING });
    vi.useFakeTimers();
    try {
      host.connections = () => new Promise(() => {});
      const outcome = program.diagnostics().catch((e: unknown) => e);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(await outcome).toMatchObject({ code: 'storage/host-silent' });
    } finally {
      vi.useRealTimers();
      program.close();
    }
  });
});
