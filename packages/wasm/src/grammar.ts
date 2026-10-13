/**
 * The grammar of Fossil for a request's names: the prelude the request already says — the types
 * imported and declared, the sources bound — and the grammar of the mappings after it, the EBNF a
 * constrained decoder (WebLLM's xgrammar) holds a model to, whose only names are the sources'
 * columns, the types and their fields and the catalogue's `parse.*` functions. The program it
 * writes is Fossil, and the checker is its oracle.
 */
import { grammarFor as raw } from '../pkg/fossil_wasm.js';
import type { Vocabulary } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/** The prelude, and the EBNF of what follows it. */
export interface Grammar {
  prelude: string;
  grammar: string;
}

/** The grammar over `vocabulary`. Boots the module itself; `signal` stops the wait for the boot. */
export async function grammarFor(vocabulary: Vocabulary, { signal }: { signal?: AbortSignal } = {}): Promise<Grammar> {
  await until(initFossilWasm(), signal);
  return raw(vocabulary) as Grammar;
}
