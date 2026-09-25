/**
 * What keeps the prompt true. Each guard names the artefact it is held to and says, on failure,
 * what to do — because the fix is always a person re-reading the prose, never a regenerated file.
 */
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { beforeAll, describe, expect, it } from 'vitest';
import type { SourceHost } from '@fossil-lang/types';
import { initFossilWasm, openProgram } from '@fossil-lang/wasm';

import { EXAMPLE, FORBIDDEN, FOSSIL_PROMPT, NAMES } from '../src/index.js';
import { GRAMMAR_DIGEST, SURFACE } from '../src/surface.js';

const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const PROGRAMS = join(ROOT, 'apps/docs/programs');

/** Every conformance program, recursively — `crates/fossil-cli/tests/programs.rs` walks the same tree. */
function programs(dir = PROGRAMS): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return programs(path);
    return name.endsWith('.fossil') ? [readFileSync(path, 'utf8')] : [];
  });
}

const REGION = /^\s*\/\/ #(end)?region\b/;

describe('the prompt is held to the language', () => {
  it('was last read against this grammar.bnf', () => {
    const digest = createHash('sha256').update(readFileSync(join(ROOT, 'grammar.bnf'))).digest('hex');
    expect(
      digest,
      'grammar.bnf changed. Re-read src/surface.ts (SURFACE and FORBIDDEN) against it, fix what ' +
        `no longer holds, then set GRAMMAR_DIGEST to '${digest}'.`,
    ).toBe(GRAMMAR_DIGEST);
  });

  it('shows the shop conformance program as its complete example, verbatim', () => {
    const shop = readFileSync(join(PROGRAMS, 'shop/shop.fossil'), 'utf8')
      .split('\n')
      .filter((line) => !REGION.test(line))
      .join('\n')
      .trimEnd();
    expect(EXAMPLE).toBe(shop);
  });

  it('shows no fossil line that no conformance program writes', () => {
    const written = new Set(programs().flatMap((p) => p.split('\n').map((l) => l.trim())));
    const fences = [...SURFACE.matchAll(/```fossil\n([\s\S]*?)```/g)].map((m) => m[1]!);
    expect(fences.length).toBeGreaterThan(0);
    const unwritten = fences
      .flatMap((f) => f.split('\n').map((l) => l.trim()))
      .filter((l) => l !== '' && !written.has(l));
    expect(unwritten, 'these lines are in the prompt and in no program under apps/docs/programs').toEqual(
      [],
    );
  });

  it('names no library function the catalogue does not declare', () => {
    const named = new Set(NAMES);
    const heads = new Set(NAMES.map((n) => n.split('.')[0]!));
    const cited = [...SURFACE.matchAll(/\b([a-z]+)\.([a-z_]+)\(/g)]
      .filter(([, head]) => heads.has(head!) || ['validate', 'anon', 'rdf'].includes(head!))
      .map(([, head, member]) => `${head}.${member}`);
    expect(cited.length).toBeGreaterThan(0);
    expect(cited.filter((n) => !named.has(n))).toEqual([]);
  });

  it('carries the generated library', () => {
    for (const name of NAMES) expect(FOSSIL_PROMPT).toContain(`\`${name}(`);
  });
});

describe('every form the prompt calls gone is refused by the checker', () => {
  // A shape and a source the preamble names, so an error is the forbidden form's and not a
  // missing document's: the control below checks clean against the same two.
  const SHAPE = 'PREFIX ex: <http://example.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nex:Person { ex:name xsd:string }\n';
  const host: SourceHost = {
    connections: async () => ({}),
    sign: async (locators) => Object.fromEntries(locators.map((l) => [l, `mem://${l}`])),
  };
  const fetchShape = (async (url: string) =>
    url.endsWith('person.shex') ? new Response(SHAPE) : new Response('', { status: 404 })) as typeof fetch;

  const errors = async (text: string): Promise<string[]> => {
    const program = await openProgram('forbidden.fossil', { host, fetch: fetchShape });
    try {
      program.registerDescriptor({
        uri: 'users.csv',
        columns: [
          { name: 'id', primitive: 'string' },
          { name: 'name', primitive: 'string' },
          { name: 'age', primitive: 'integer' },
        ],
        freshness_token: '',
      });
      return (await program.check(text)).filter((r) => r.severity === 1).map((r) => r.message);
    } finally {
      program.close();
    }
  };

  beforeAll(async () => {
    const wasm = fileURLToPath(new URL('../../wasm/pkg/fossil_wasm_bg.wasm', import.meta.url));
    await initFossilWasm(await readFile(wasm));
  });

  it('control: the preamble alone, and with a well-formed mapping, checks clean', async () => {
    expect(await errors(CONTROL.split('People')[0]!)).toEqual([]);
    expect(await errors(CONTROL)).toEqual([]);
  });

  for (const forbidden of FORBIDDEN) {
    it(forbidden.form, async () => {
      expect(
        await errors(forbidden.refused),
        `the checker accepts ${forbidden.form}; the prompt must not call it gone`,
      ).not.toEqual([]);
    });
  }
});

const CONTROL = `type { Person } := io.shex("person.shex")
User := io.csv("users.csv")
People : Person from User
    @subject = "https://example.org/{User.id}"
    name = User.name
`;
