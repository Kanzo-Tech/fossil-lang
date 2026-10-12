/**
 * What a model may write once a program's sources are profiled: the prelude the request already
 * says, and the grammar of the mappings after it — the EBNF a constrained decoder (WebLLM's
 * xgrammar) holds the model to, whose only names are the sources' columns, the shapes' types and
 * properties and the catalogue's `parse.*` functions. The program it writes is Fossil, and the
 * checker is its oracle.
 */
import { mappingGrammar as raw } from '../pkg/fossil_wasm.js';
import type { MappingVocabulary } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/** The prelude, and the EBNF of what follows it. */
export interface MappingGrammar {
  prelude: string;
  grammar: string;
}

/** The grammar over `vocabulary`. Boots the module itself; `signal` stops the wait for the boot. */
export async function mappingGrammar(vocabulary: MappingVocabulary, { signal }: { signal?: AbortSignal } = {}): Promise<MappingGrammar> {
  await until(initFossilWasm(), signal);
  return raw(vocabulary) as MappingGrammar;
}
