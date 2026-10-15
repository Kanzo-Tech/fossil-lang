/**
 * `program.mapping()` is the RML-Core mapping a program's types describe, before any run, across the
 * real wasm boundary. The mapping's content is `fossil-mir`'s and `crates/fossil-df/tests/mapping.rs`
 * holds it to the one a run writes; this holds that the export reaches JS, reads the types the program
 * declares, and needs no profile.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { initFossilWasm, openProgram } from '../src/index.js';
import { recordingHost } from './host.js';

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
});

const PROGRAM = `@iri("https://schema.org/Person")
type Laureate
    @iri("https://schema.org/name") name: String
    @iri("https://nobel.example/vocab/awarded") awarded: Seq<Ref<Prize>>

@iri("https://nobel.example/vocab/Prize")
type Prize
    @iri("https://nobel.example/vocab/category") category: String
`;

describe('FossilProgram.mapping', () => {
  it('describes the declared types as RML before anything is read', async () => {
    const { host, fetched } = recordingHost('{}');
    const program = await openProgram('declared.fossil', { host, text: PROGRAM });
    try {
      const turtle = await program.mapping();
      expect(fetched).toEqual([]);
      expect(turtle).toContain('https://schema.org/Person');
      expect(turtle).toContain('https://schema.org/name');
      expect(turtle).toContain('Person_awarded_Prize');
      expect(turtle).toContain('<#Prize>');
    } finally {
      program.close();
    }
  });

  it('answers the mapping of the text of the last update', async () => {
    const { host } = recordingHost('{}');
    const program = await openProgram('declared.fossil', { host, text: PROGRAM });
    try {
      program.update(PROGRAM.replace('category: String', 'kind: String').replace('vocab/category', 'vocab/kind'));
      const turtle = await program.mapping();
      expect(turtle).toContain('https://nobel.example/vocab/kind');
      expect(turtle).not.toContain('https://nobel.example/vocab/category');
    } finally {
      program.close();
    }
  });

  it('describes a program with no type as an empty mapping, and refuses an IRI two columns share', async () => {
    const { host } = recordingHost('{}');
    const program = await openProgram('declared.fossil', { host, text: '' });
    try {
      expect(await program.mapping()).not.toContain('Person');
      program.update(PROGRAM.replace('vocab/awarded', 'name').replace('https://schema.org/name', 'https://nobel.example/name'));
      await expect(program.mapping()).rejects.toMatchObject({ code: 'write/ambiguous-iri' });
    } finally {
      program.close();
    }
  });
});
