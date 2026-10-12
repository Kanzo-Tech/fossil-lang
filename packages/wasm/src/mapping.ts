/**
 * The program a mapping spec is: `fossil-mapping`'s emitter, in the module. A model fills the spec
 * under its JSON Schema and never writes Fossil; this writes it, and the program goes through the
 * same checker as one written by hand.
 */
import { emitMapping as rawEmit } from '../pkg/fossil_wasm.js';
import type { MappingSpec } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/** What a spec emits: the program, or where in the spec no program could say it (a JSON Pointer). */
export type Emitted = { program: string } | { at: string; message: string };

/** Emit `spec`. Boots the module itself; `signal` stops the wait for the boot. */
export async function emitMapping(spec: MappingSpec, { signal }: { signal?: AbortSignal } = {}): Promise<Emitted> {
  await until(initFossilWasm(), signal);
  return rawEmit(spec) as Emitted;
}
