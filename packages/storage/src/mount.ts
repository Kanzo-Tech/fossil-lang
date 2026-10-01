import {
  FossilError,
  HOST_MS,
  attachCause,
  isFossilError,
  until,
  within,
  type Access,
  type Engine,
  type Host,
  type Scope,
  type StorageCredential,
} from '@fossil-lang/types';

import { silent } from './documents.js';
import { covering, initStorage, nameOf, plan } from './wasm.js';

/** A scope made readable through the engine, until {@link Mount.close}. */
export interface Mount {
  /** The prefixes the host vended for the scope. */
  readonly prefixes: readonly string[];
  /** What SQL calls `locator`: `s3://…` itself, or the name an Azure file is lent under. */
  name(locator: string): string;
  /** {@link name} for each locator, each made readable first — an Azure file is lent. */
  files(locators: readonly string[]): Promise<string[]>;
  /**
   * Why a credential of this mount stopped working: its renewal failed until it expired. Set, it is
   * what {@link name} and {@link files} throw, and what a reader reports in place of the engine's
   * 403 — the storage failure, with the host's own error as its cause.
   */
  readonly failure: FossilError | undefined;
  /**
   * Give the credentials back: the last holder of a prefix drops its secret and its leases. Every
   * prefix is released even when one fails, and the failures are thrown after, as one.
   */
  close(): Promise<void>;
}

/** Renew this long before a credential expires — Iceberg's `VendedCredentialsProvider` margin. */
export const RENEW_BEFORE_MS = 5 * 60_000;
/** A renewal that failed is tried again after this, while the credential still works. */
export const RETRY_MS = 30_000;

interface Held {
  credential: StorageCredential;
  readonly scope: Scope;
  readonly access: Access;
  holders: number;
  ready: Promise<void>;
  /** Locator → the name it is lent under, for a store the engine is lent file by file. */
  readonly lent: Map<string, string>;
  timer?: ReturnType<typeof setTimeout>;
  /** Why the credential stopped working, once its renewal failed past expiry. */
  failure?: FossilError;
}

const held = new WeakMap<Engine, Map<string, Held>>();
const httpfs = new WeakSet<Engine>();

/**
 * Put the credentials the host vends for `scope` into `engine`, and keep them fresh.
 *
 * Each S3 prefix becomes one `CREATE OR REPLACE SECRET` scoped to it; a renewal re-issues the same
 * statement under the same name at `expires − 5 min`, so a view never reopens. Two mounts of one
 * prefix on one engine share the secret, and the last to close drops it. An Azure prefix is lent
 * file by file, because DuckDB-WASM has no Azure extension: no glob there, and the readers never
 * glob — a manifest enumerates.
 *
 * `signal` stops it, rejecting with the signal's reason. The statements that install a credential are
 * not interrupted — another mount may be waiting on the same one — only the wait for them.
 *
 * @throws {FossilError} `storage/host-refused` when the host rejects the request, `storage/host-silent`
 *   when it does not answer within 30 s, `storage/no-credential` when it vends nothing for the scope,
 *   `storage/no-httpfs` when the engine has no `httpfs`, `engine/failed` when the engine refuses a
 *   secret.
 */
export async function mount(
  engine: Engine,
  host: Host,
  scope: Scope,
  access: Access,
  { signal }: { signal?: AbortSignal } = {},
): Promise<Mount> {
  await until(initStorage(), signal);
  const credentials = await vended(host, scope, access, signal);
  if (credentials.length === 0) throw noCredential(scope, access);
  const table = held.get(engine) ?? new Map<string, Held>();
  held.set(engine, table);
  const keys: string[] = [];
  try {
    for (const credential of credentials) {
      const key = `${access}\u0000${credential.prefix}`;
      let entry = table.get(key);
      if (entry) {
        entry.holders++;
      } else {
        const fresh: Held = { credential, scope, access, holders: 1, ready: Promise.resolve(), lent: new Map() };
        fresh.ready = install(engine, fresh).then(() => schedule(engine, host, fresh));
        table.set(key, (entry = fresh));
      }
      keys.push(key);
      await until(entry.ready, signal);
    }
  } catch (cause) {
    try {
      await release(engine, table, keys);
    } catch (cleanup) {
      throw attachCause(cause, cleanup);
    }
    throw cause;
  }

  const current = (): StorageCredential[] => keys.map((key) => table.get(key)!.credential);
  const entryFor = (locator: string): Held => {
    const credential = covering(current(), locator);
    if (credential === undefined) {
      const prefix = current()
        .map((c) => c.prefix)
        .join(', ');
      throw FossilError.of(
        'storage/outside-prefix',
        { locator, prefix },
      );
    }
    const entry = table.get(`${access}\u0000${credential.prefix}`)!;
    if (entry.failure !== undefined) throw entry.failure;
    return entry;
  };
  let closed = false;

  return {
    prefixes: credentials.map((c) => c.prefix),
    get failure() {
      return keys.map((key) => table.get(key)?.failure).find((f) => f !== undefined);
    },
    name: (locator) => nameOf(entryFor(locator).credential, locator).name,
    async files(locators) {
      const leases: Record<string, string> = {};
      const names = locators.map((locator) => {
        const entry = entryFor(locator);
        const { name, lend } = nameOf(entry.credential, locator);
        if (lend !== null) {
          entry.lent.set(locator, name);
          leases[name] = lend;
        }
        return name;
      });
      if (Object.keys(leases).length > 0) await engine.lend(leases);
      return names;
    },
    async close() {
      if (closed) return;
      closed = true;
      await release(engine, table, keys);
    },
  };
}

/** A signal nothing aborts: what a statement fossil runs for itself — a cleanup, a renewal — carries. */
const unstoppable = (): AbortSignal => new AbortController().signal;

async function release(engine: Engine, table: Map<string, Held>, keys: readonly string[]): Promise<void> {
  const failures: unknown[] = [];
  for (const key of keys) {
    const entry = table.get(key);
    if (entry === undefined || --entry.holders > 0) continue;
    table.delete(key);
    clearTimeout(entry.timer);
    // An install still in flight would put the secret back after the uninstall; one that failed
    // has already been reported to whoever was waiting on it.
    await entry.ready.then(
      () => undefined,
      () => undefined,
    );
    const { uninstall } = plan(entry.credential, entry.access);
    try {
      if (uninstall !== null) await engine.query(uninstall, { signal: unstoppable() });
    } catch (cause) {
      failures.push(cause);
    }
    try {
      if (entry.lent.size > 0) await engine.drop([...entry.lent.values()]);
    } catch (cause) {
      failures.push(cause);
    }
  }
  if (failures.length > 0) {
    throw FossilError.of('engine/failed', {}, {
      cause: failures.length === 1 ? failures[0] : new AggregateError(failures, `${failures.length} releases failed`),
    });
  }
}

async function install(engine: Engine, entry: Held): Promise<void> {
  const { install: sql } = plan(entry.credential, entry.access);
  if (sql !== null) {
    await requireHttpfs(engine);
    try {
      await engine.query(sql, { signal: unstoppable() });
    } catch (cause) {
      throw FossilError.of('engine/failed', {}, { cause });
    }
  }
  if (entry.lent.size > 0) {
    const leases: Record<string, string> = {};
    for (const [locator, name] of entry.lent) leases[name] = nameOf(entry.credential, locator).lend!;
    await engine.lend(leases);
  }
}

function schedule(engine: Engine, host: Host, entry: Held, delay?: number): void {
  const { expiresAtMs } = plan(entry.credential, entry.access);
  if (expiresAtMs === null) return;
  const wait = delay ?? Math.max(0, expiresAtMs - RENEW_BEFORE_MS - Date.now());
  entry.timer = setTimeout(() => void renew(engine, host, entry), wait);
}

async function renew(engine: Engine, host: Host, entry: Held): Promise<void> {
  if (entry.holders === 0) return;
  const { prefix } = entry.credential;
  try {
    const fresh = (await vended(host, entry.scope, entry.access, undefined)).find((c) => c.prefix === prefix);
    if (fresh === undefined) throw noCredential(entry.scope, entry.access);
    if (entry.holders === 0) return;
    entry.credential = fresh;
    await install(engine, entry);
    schedule(engine, host, entry);
  } catch (cause) {
    const { expiresAtMs } = plan(entry.credential, entry.access);
    if (expiresAtMs !== null && expiresAtMs > Date.now()) {
      schedule(engine, host, entry, RETRY_MS);
      return;
    }
    // Nobody is waiting on a renewal, so its failure is kept where the next read finds it.
    entry.failure = isFossilError(cause)
      ? cause
      : FossilError.of('internal/bug', { what: `renewing ${prefix} failed outside fossil` }, {
          cause,
        });
  }
}

async function requireHttpfs(engine: Engine): Promise<void> {
  if (httpfs.has(engine)) return;
  let answer: Awaited<ReturnType<Engine['query']>>;
  try {
    answer = await engine.query("SELECT loaded FROM duckdb_extensions() WHERE extension_name = 'httpfs'", {
      signal: unstoppable(),
    });
  } catch (cause) {
    throw FossilError.of('engine/failed', {}, { cause });
  }
  if (answer.numRows === 0 || answer.getChild('loaded')?.get(0) !== true) {
    throw FossilError.of(
      'storage/no-httpfs',
      {},
      { help: "fossil reads s3:// through a scoped secret, and the host's engine loads httpfs before handing it over" },
    );
  }
  httpfs.add(engine);
}

async function vended(
  host: Host,
  scope: Scope,
  access: Access,
  signal: AbortSignal | undefined,
): Promise<StorageCredential[]> {
  const said = describe(scope);
  try {
    return await within(HOST_MS, (bounded) => host.credentials(scope, access, { signal: bounded }), {
      signal,
      silent: (after) => silent(said, after),
    });
  } catch (cause) {
    if (isFossilError(cause, 'storage/host-silent') || signal?.aborted) throw cause;
    throw FossilError.of('storage/host-refused', { scope: said }, { cause });
  }
}

function noCredential(scope: Scope, access: Access): FossilError<'storage/no-credential'> {
  const said = describe(scope);
  return FossilError.of(
    'storage/no-credential',
    { scope: said, access },
  );
}

export function describe(scope: Scope): string {
  return 'job' in scope ? `job ${scope.job}` : `connection ${scope.connection}`;
}
