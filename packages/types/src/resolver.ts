/**
 * Source resolution types for the Fossil playground React library family.
 *
 * Source resolution is two-tier, through a host-injected `ConnectionResolver`:
 * the React component never sees plaintext credentials. The host supplies a
 * `ConnectionResolver` implementation that maps logical `@connector/path`
 * references to browser-fetchable URLs at execution time.
 *
 * The same `@connector/path` grammar serves both tiers:
 *   - Tier 1 (browser-local): bundled examples + uploads + public CORS HTTPS
 *     — see `createDefaultResolver` in `@fossil-lang/resolvers`.
 *   - Tier 2 (host-mediated): host injects a custom resolver (Keasy vault,
 *     paper-demo fixture, AWS Secrets Manager, etc.).
 *
 * These types are the IoC contract — every package in the @fossil-lang/*
 * family that touches sources imports them from here.
 */

/**
 * Parsed `@connector/path` reference.
 *
 * Connector-name validation rule (borrowed from Keasy):
 * `/^[a-z0-9][a-z0-9_-]*$/i` — applied at parse time. Resolvers MUST reject
 * invalid names with a clear error rather than silently coercing.
 */
export interface SourceRef {
  /** Raw text after `@`, e.g. `"my-conn/file.parquet"`. */
  raw: string;
  /** The connector name (left of `/`), e.g. `"my-conn"`. */
  connector: string;
  /** The path within the connector (right of `/`), e.g. `"file.parquet"`. */
  path: string;
}

/**
 * The fetchable shape the component substitutes into codegen'd SQL at run
 * time. Hosts that can provide schema hints do; hosts that cannot leave
 * the field undefined (Tier-1 default resolvers typically don't have them).
 */
export interface ResolvedSource {
  /** Browser-fetchable URL (`blob:`, `https:`, `file:`, etc.). The component
   *  substitutes this into the codegen'd SQL at run time. */
  url: string;
  /** Optional codegen hint — the `io.{csv,json,parquet}` constructor to use. */
  format?: 'csv' | 'json' | 'parquet';
  /** Optional schema hint for IDE preview (autocomplete for shape properties etc.).
   *  Tier-1 resolvers typically don't provide this; Tier-2 hosts can. */
  schema?: SourceSchema;
}

/**
 * A minimal schema description — sufficient for IDE preview features
 * (autocomplete on shape properties, goto-source). Hosts that have richer
 * type information may extend this in a future major.
 */
export interface SourceSchema {
  fields: Array<{ name: string; type: string; nullable?: boolean }>;
}

/**
 * Tier-1 connector kinds advertised by the default resolver. Tier-2 hosts
 * (Keasy etc.) may advertise additional logical kinds outside this enum,
 * but the surveyed Tier-1 set is closed.
 */
export type ConnectorType = 'local_file' | 'public_http' | 'upload' | 'examples';

/**
 * Connector advertised by `ConnectionResolver.list()`. Drives the editor's
 * `@`-autocomplete + the host's UI (sidebar list, manager dialog, etc.).
 */
export interface Connector {
  /** Connector name (matches the `@connector/` prefix in mappings).
   *  Naming rule: `/^[a-z0-9][a-z0-9_-]*$/i`. */
  name: string;
  /** Classification — drives the icon + UI affordances. */
  type: ConnectorType;
  /** Optional human-readable label for UIs. */
  label?: string;
}

/**
 * Event emitted by a resolver's optional `subscribe()` channel. Consumers
 * (the editor's `@`-autocomplete cache, the result panel) invalidate any
 * cached state for the affected connector when one of these fires.
 */
export interface ResolverEvent {
  type: 'connector-added' | 'connector-removed' | 'connector-updated';
  /** The affected connector name. */
  connector: string;
}

/**
 * The IoC contract — every host implements this; every consumer accepts it.
 *
 * The component NEVER sees plaintext credentials. Resolver
 * implementations bear the responsibility of:
 *   - Validating connector names against the `SourceRef` naming rule above.
 *   - NOT retaining credential-shape strings in any state exposed via
 *     `list()` or returned from `resolve()`. The CONN-01 invariant test
 *     in `@fossil-lang/resolvers/tests/no-credentials-leak.test.ts` asserts
 *     this property structurally.
 *   - Treating presigned URLs as ephemeral — embed them in the
 *     `ResolvedSource.url` and do not cache the URL string anywhere a
 *     downstream consumer can read it back.
 */
export interface ConnectionResolver {
  /** Resolve a `@connector/path` reference to a fetchable URL. */
  resolve(ref: SourceRef): Promise<ResolvedSource>;
  /** List known connectors — drives the editor's `@`-autocomplete + UI. */
  list(): Promise<Connector[]>;
  /** Optional host UI hook — opens a connector manager dialog. */
  openManager?(): void;
  /** Optional cache-invalidation subscription. Returns an unsubscribe fn. */
  subscribe?(listener: (event: ResolverEvent) => void): () => void;
}
