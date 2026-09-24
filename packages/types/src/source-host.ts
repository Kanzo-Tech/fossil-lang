/**
 * What a host gives every `@fossil-lang/*` package that reads a program's
 * sources — the checker, introspection and the executor accept this and
 * nothing else.
 *
 * It is a capability, not a resolver. Fossil decides which documents and
 * sources a program needs and turns each `@conn/path` into a locator through
 * `connections()`; the host never parses a reference. `sign` is the one step
 * that needs the host's credentials: it maps each locator to a URL the browser
 * can fetch, and a locator it will not sign is absent from the answer.
 */
export interface SourceHost {
  /** Connection name → base URL, as `@name/…` expands against it. */
  connections(): Promise<Record<string, string>>;
  /** Locator → fetchable URL, for each locator the host signs. */
  sign(locators: string[]): Promise<Record<string, string>>;
}
