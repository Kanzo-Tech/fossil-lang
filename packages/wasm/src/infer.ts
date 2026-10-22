/**
 * The {@link Model} a {@link Profile} implies: W3C's Direct Mapping with the bridge rule D2RQ and
 * Ontop add — a source with a key a type, an inclusion on a key a relation, a keyless source between
 * two types a link file — and every question the profile cannot settle, each standing at an answer.
 * The rules are `fossil_ide::model::infer`'s; {@link write} makes the model a program.
 */
import { infer as raw } from '../pkg/fossil_wasm.js';
import type { Inferred, Profile } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/**
 * The model `profile` implies. `choices` answers an ambiguity by its `id` with one of its `options`;
 * one not answered stands at its first, and `chosen` says which it took. Boots the module itself;
 * `signal` stops the wait for the boot.
 */
export async function infer(
  profile: Profile,
  { choices = {}, signal }: { choices?: Readonly<Record<string, string>>; signal?: AbortSignal } = {},
): Promise<Inferred> {
  await until(initFossilWasm(), signal);
  return raw(profile, choices);
}
