/**
 * A storage credential scoped to one prefix — Iceberg REST's `StorageCredential`, verbatim on the
 * wire, so what a catalogue vends to DuckDB, Spark or PyIceberg is what a host vends to fossil.
 *
 * `config` carries Iceberg's keys: `s3.access-key-id`, `s3.secret-access-key`, `s3.session-token`,
 * `s3.endpoint`, `s3.path-style-access`, `client.region`, `s3.session-token-expires-at-ms`; or
 * `adls.sas-token.<host>` and `adls.sas-token-expires-at-ms.<host>`, `<host>` being the prefix's
 * `<account>.dfs.core.windows.net`.
 */
export interface StorageCredential {
  /** Canonical and ending in `/`: `s3://bucket/output/<job>/`, `abfss://c@acct.dfs.core.windows.net/dir/`. */
  prefix: string;
  config: Record<string, string>;
}

export type Access = 'read' | 'write';

/** What a credential is asked for: a connection's prefix, or a job's dataset. */
export type Scope = { connection: string } | { job: string };

/**
 * What a host gives every `@fossil-lang/*` package that touches storage, and nothing else.
 *
 * Fossil decides what a program reads and which connection each `@name/…` goes through; the host
 * never parses a reference. `credentials` is the one step that needs the host's authority: it vends
 * `access` on a scope, short-lived and scoped to a prefix, and the store enforces the prefix. Where
 * several credentials come back, the longest prefix that covers a file is the one used.
 */
export interface Host {
  /** Connection name → canonical prefix, as `@name/…` expands against it. */
  connections(): Promise<Record<string, string>>;
  credentials(scope: Scope, access: Access): Promise<StorageCredential[]>;
}

/** A document a program names and the workspace does not hold yet —
 *  `fossil_hir::documents::MissingDocument` across the wasm boundary. */
export interface MissingDocument {
  /** The registry key: what the program wrote, independent of any connection. */
  key: string;
  /** Where to read it: the key expanded through the connection map. */
  locator: string;
  /** The connection the locator lies under, when the program wrote `@name/…`. */
  connection?: string;
}

/** A data source a program reads, as fossil resolved it — what introspection
 *  DESCRIBEs. `key` is what the program wrote, and the descriptor is
 *  registered under it; `locator` is what gets read. */
export interface ProgramSource {
  /** The binding the source is read into (`users` in `users := io.csv(…)`). */
  binding: string;
  key: string;
  locator: string;
  /** The connection the locator lies under, when the program wrote `@name/…`. */
  connection?: string;
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

/** What a compiled workspace answers — the checker's and the executor's. */
export interface DocumentWorkspace {
  /** The map `@name/…` expands against. It moves locators, never keys. */
  setConnections(connections: Record<string, string>): void;
  missingDocuments(): MissingDocument[];
  registerDocument(key: string, text: string): void;
}
