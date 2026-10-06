import type { Problem } from './error.js';
import type { Access, MissingDocument, Scope, StorageCredential } from './wire.gen.js';

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
  connections(options: HostCall): Promise<Record<string, string>>;
  credentials(scope: Scope, access: Access, options: HostCall): Promise<StorageCredential[]>;
}

/**
 * What every {@link Host} call is handed: a signal that aborts when fossil stops waiting — the
 * caller's Stop, or the 30 s a host has to answer (`HOST_MS`). A host that passes it to its `fetch`
 * stops the request too; one that ignores it is simply no longer waited for, and a host that never
 * answers is `storage/host-silent`.
 */
export interface HostCall {
  readonly signal: AbortSignal;
}

/** A document that stayed missing, and why — the problem its read answered. The checker reports
 *  it as a diagnostic on its own; a run treats it as a failure. */
export interface UnreadDocument extends MissingDocument {
  problem: Problem;
}

/** What a compiled workspace answers — the checker's and the executor's. */
export interface DocumentWorkspace {
  /** The map `@name/…` expands against. It moves locators, never keys. */
  setConnections(connections: Record<string, string>): void;
  missingDocuments(): MissingDocument[];
  registerDocument(key: string, text: string): void;
}
