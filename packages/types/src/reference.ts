/**
 * The reference a program writes for a locator — the inverse of the one expansion fossil makes.
 *
 * A program writes `@lake/users.csv`, and `fossil_locator` expands it through `Host.connections()`'
 * map into `<base>/users.csv`: the connection's base, its trailing `/` trimmed, a `/`, and the path.
 * A host that lists a connection's files holds the other end — a URL — and needs what to write.
 */

/**
 * What a program writes to read `locator`: `@name/path` through the connection whose base covers it,
 * the longest base when several do, and `locator` itself when none does — which is what fossil reads
 * a reference with a scheme or an absolute path as.
 *
 * Expanding the answer through the same `connections` gives `locator` back — for any locator not itself
 * written `@…`, which no expansion produces. A base covers a
 * locator when the locator starts with the base, its trailing `/` trimmed, and a `/`. A connection
 * whose name holds a `/` is never chosen, since `@name/…` ends the name at the first one.
 *
 * ```ts
 * referenceTo('s3://lake/in/users.csv', { lake: 's3://lake/in/' }) // '@lake/users.csv'
 * referenceTo('s3://other/x.csv', { lake: 's3://lake/in/' })       // 's3://other/x.csv'
 * ```
 */
export function referenceTo(locator: string, connections: Readonly<Record<string, string>>): string {
  let best: { name: string; base: string } | undefined;
  for (const [name, prefix] of Object.entries(connections)) {
    if (name.includes('/')) continue;
    const base = prefix.replace(/\/+$/, '');
    if (!locator.startsWith(`${base}/`)) continue;
    if (best === undefined || base.length > best.base.length) best = { name, base };
  }
  return best === undefined ? locator : `@${best.name}/${locator.slice(best.base.length + 1)}`;
}
