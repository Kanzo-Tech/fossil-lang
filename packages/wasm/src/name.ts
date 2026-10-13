/**
 * The binding name a file becomes in a program: `people` for `people.csv`, `crm_people` for
 * `crm/people.csv`, `_2024_sales` for `2024-sales.parquet` — the extension gone, every character an
 * identifier cannot hold `_`, and `_` before a name that would not lex as one `IDENT` (a leading
 * digit, a reserved word). The rule is the lexer's (`fossil_syntax::name::binding_name`), so what
 * comes back always reads as one identifier.
 */
import { bindingName as raw } from '../pkg/fossil_wasm.js';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/** The binding name `file` becomes. Boots the module itself; `signal` stops the wait for the boot. */
export async function bindingName(file: string, { signal }: { signal?: AbortSignal } = {}): Promise<string> {
  await until(initFossilWasm(), signal);
  return raw(file);
}
