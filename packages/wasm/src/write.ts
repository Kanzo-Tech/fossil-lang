/**
 * The program a {@link Model} implies: what a host decided a graph holds — the sources it reads,
 * the types over them, each field's column and the relations between them — written as Fossil in
 * one canonical layout, every IRI under the workspace's base. The rules are `fossil_ide::model`'s;
 * the program is checked like any other.
 */
import { write as raw } from '../pkg/fossil_wasm.js';
import type { Model } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/**
 * The program `model` implies, every IRI under `base` (`https://acme.example/` makes
 * `https://acme.example/vocab/Laureates`): each type declared, each source bound, one mapping per
 * type and one more per relation read across a link file. The same model writes the same text.
 * Boots the module itself; `signal` stops the wait for the boot.
 */
export async function write(model: Model, { base, signal }: { base: string; signal?: AbortSignal }): Promise<string> {
  await until(initFossilWasm(), signal);
  return raw(model, base);
}
