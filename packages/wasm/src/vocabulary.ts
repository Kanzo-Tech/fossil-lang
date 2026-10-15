/**
 * The names a model's mappings are written in, computed from what a host decided and not proposed
 * by whoever writes the mappings: the {@link Vocabulary} of a {@link Model} over its profiled
 * sources — each type declared, every IRI under the workspace's base — and each type's `@subject`.
 * {@link grammarFor} takes the vocabulary, and the subject is the one line of a mapping a model is
 * not asked to write.
 */
import { subjectOf as rawSubject, vocabularyOf as rawVocabulary } from '../pkg/fossil_wasm.js';
import type { Model, ModelType, SourceVocabulary, Vocabulary } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/**
 * The vocabulary of `model` over `sources`, every IRI under `base` (`https://acme.example/` makes
 * `https://acme.example/vocab/Laureate`): each type declared with a field per kept column and one per
 * relation that leaves it, `Option<…>` where some rows have none and `Seq<Ref<…>>` across a link.
 * Of `sources` only the ones a type or a relation reads are kept. Boots the module itself; `signal`
 * stops the wait for the boot.
 */
export async function vocabularyOf(
  model: Model,
  sources: readonly SourceVocabulary[],
  base: string,
  { signal }: { signal?: AbortSignal } = {},
): Promise<Vocabulary> {
  await until(initFossilWasm(), signal);
  return rawVocabulary(model, [...sources], base);
}

/**
 * A type's `@subject` under `base`: the things' IRI prefix of the workspace and of the type, then
 * the columns of its identity as holes — `https://acme.example/id/laureate/{laureates.id}`. Boots
 * the module itself; `signal` stops the wait for the boot.
 */
export async function subjectOf(
  type: ModelType,
  base: string,
  { signal }: { signal?: AbortSignal } = {},
): Promise<string> {
  await until(initFossilWasm(), signal);
  return rawSubject(type, base);
}
