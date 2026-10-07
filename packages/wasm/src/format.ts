/**
 * Which formats this build reads, and which of them reads a file — the question a host asks
 * whenever it lists files for a person to pick: which of these can a program read as data, which
 * as a schema, and with which `io.` constructor.
 */
import { formats as rawFormats } from '../pkg/fossil_wasm.js';
import type { Format, Role } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/**
 * The formats fossil reads (`csv`, `rdf`, `shex`, …): the name, the extensions it reads, and how it
 * can be used. Compiled into the module, so a host may keep the answer for as long as it keeps the
 * module. Boots the module itself; `signal` stops the wait for the boot.
 */
export async function formats({ signal }: { signal?: AbortSignal } = {}): Promise<Format[]> {
  await until(initFossilWasm(), signal);
  return rawFormats();
}

/**
 * The format that reads `path` in `role`, by its extension, case-insensitively — or `undefined`
 * when none does. Pure: it reads the list {@link formats} answered, so a host filtering a listing
 * of a thousand files asks the module once.
 *
 * `role` is not optional because one extension can name two formats: a `.ttl` is `rdf` read as
 * data and `shacl` read as a schema. A format whose `kind` is `both` answers either.
 *
 * `path` is anything a program could write as a source — a bare name, a `@conn/…` reference, a
 * URL. A URL's query and fragment are not part of its extension (`…/a.csv?sig=…` is a CSV), and a
 * name with no extension (`README`, `.env`) has none, so no format reads it.
 */
export function formatFor(path: string, role: Role, formats: readonly Format[]): Format | undefined {
  const cut = path.search(/[?#]/);
  const name = (cut < 0 ? path : path.slice(0, cut)).split('/').pop() ?? '';
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return undefined;
  const extension = name.slice(dot + 1).toLowerCase();
  return formats.find((f) => (f.kind === role || f.kind === 'both') && f.extensions.includes(extension));
}
