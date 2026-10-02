/**
 * Which provider reads a file — the question a host asks of {@link providers} whenever it lists
 * files for a person to pick: which of these can a program read as data, which as a schema, and
 * with which `io.` constructor.
 *
 * Pure, and needs no module: it reads the list `providers()` answered, so a host filtering a
 * listing of a thousand files asks the module once.
 */
import type { ProviderInfo, SourceRefInfo } from './index.js';

/**
 * The provider that reads `path` in `role`, by its extension, case-insensitively — or `undefined`
 * when none does.
 *
 * `role` is {@link SourceRefInfo.role}, and it is not optional because one extension can name two
 * providers: a `.ttl` is `rdf` read as data and `shacl` read as a schema. A provider whose `kind` is
 * `both` answers either.
 *
 * `path` is anything a program could write as a source — a bare name, a `@conn/…` reference, a
 * URL. A URL's query and fragment are not part of its extension (`…/a.csv?sig=…` is a CSV), and a
 * name with no extension (`README`, `.env`) has none, so no provider reads it.
 */
export function providerFor(
  path: string,
  role: SourceRefInfo['role'],
  providers: readonly ProviderInfo[],
): ProviderInfo | undefined {
  const name = path.replace(/[?#].*$/, '').split('/').pop() ?? '';
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return undefined;
  const extension = name.slice(dot + 1).toLowerCase();
  return providers.find(
    (p) => (p.kind === role || p.kind === 'both') && p.extensions.includes(extension),
  );
}
