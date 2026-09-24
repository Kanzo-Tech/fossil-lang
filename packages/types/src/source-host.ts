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

/** A document a program names and the workspace does not hold yet —
 *  `fossil_hir::documents::MissingDocument` across the wasm boundary. */
export interface MissingDocument {
  /** The registry key: what the program wrote, independent of any connection. */
  key: string;
  /** Where to read it: the key expanded through the connection map. */
  locator: string;
}

/** A data source a program reads, as fossil resolved it — what introspection
 *  DESCRIBEs. `key` is what the program wrote, and the descriptor is
 *  registered under it; `locator` is what gets signed and read. */
export interface ProgramSource {
  /** The binding the source is read into (`users` in `users := io.csv(…)`). */
  binding: string;
  key: string;
  locator: string;
  /** The `io.` constructor — it chooses the reader. */
  format: string;
  /** The reader option the binding named (`delimiter = "|"`), verbatim. */
  option?: string;
}

/** A document that stayed missing, and why. The checker reports it as a
 *  diagnostic on its own; a run treats it as a failure. */
export interface UnreadDocument extends MissingDocument {
  reason: string;
}

/** The two calls a compiled workspace answers — the checker's and the executor's. */
export interface DocumentWorkspace {
  missingDocuments(): MissingDocument[];
  registerDocument(key: string, text: string): void;
}

/**
 * Read every document a workspace is missing through `host`, until nothing
 * new is missing: a registered document can name another.
 *
 * The one IO loop over {@link SourceHost}. Fossil says what is missing and
 * where it lives; this signs, fetches and registers. Each document is
 * attempted once, so one that cannot be read ends the loop instead of
 * repeating it.
 */
export async function resolveDocuments(
  workspace: DocumentWorkspace,
  host: SourceHost,
  fetchImpl: typeof fetch = fetch,
): Promise<{ registered: number; unread: UnreadDocument[] }> {
  const attempted = new Set<string>();
  const unread: UnreadDocument[] = [];
  let registered = 0;

  for (;;) {
    const pending = workspace.missingDocuments().filter((d) => !attempted.has(d.key));
    if (pending.length === 0) return { registered, unread };
    for (const d of pending) attempted.add(d.key);

    const signed = await host.sign(pending.map((d) => d.locator));
    await Promise.all(
      pending.map(async (d) => {
        const url = signed[d.locator];
        if (!url) {
          unread.push({ ...d, reason: 'the host does not sign this locator' });
          return;
        }
        const res = await fetchImpl(url);
        if (!res.ok) {
          unread.push({ ...d, reason: `HTTP ${res.status}` });
          return;
        }
        workspace.registerDocument(d.key, await res.text());
        registered++;
      }),
    );
  }
}
