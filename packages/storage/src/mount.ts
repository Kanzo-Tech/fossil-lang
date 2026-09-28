import type { Access, Engine, Host, Scope, StorageCredential } from '@fossil-lang/types';

import { covering, initStorage, nameOf, plan } from './wasm.js';

/** A scope made readable through the engine, until {@link Mount.close}. */
export interface Mount {
  /** The prefixes the host vended for the scope. */
  readonly prefixes: readonly string[];
  /** What SQL calls `locator`: `s3://…` itself, or the name an Azure file is lent under. */
  name(locator: string): string;
  /** {@link name} for each locator, each made readable first — an Azure file is lent. */
  files(locators: readonly string[]): Promise<string[]>;
  /** Give the credentials back: the last holder of a prefix drops its secret and its leases. */
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
 * @throws when the host vends nothing for the scope, or the engine has no `httpfs`.
 */
export async function mount(
  engine: Engine,
  host: Host,
  scope: Scope,
  access: Access,
): Promise<Mount> {
  await initStorage();
  const credentials = await host.credentials(scope, access);
  if (credentials.length === 0) {
    throw new Error(`the host vended no ${access} credential for ${describe(scope)}`);
  }
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
      await entry.ready;
    }
  } catch (cause) {
    await release(engine, table, keys);
    throw cause;
  }

  const current = (): StorageCredential[] => keys.map((key) => table.get(key)!.credential);
  const entryFor = (locator: string): Held => {
    const credential = covering(current(), locator);
    if (credential === undefined) {
      throw new Error(
        `${locator} lies outside what the host vended for ${describe(scope)} (${current()
          .map((c) => c.prefix)
          .join(', ')})`,
      );
    }
    return table.get(`${access}\u0000${credential.prefix}`)!;
  };
  let closed = false;

  return {
    prefixes: credentials.map((c) => c.prefix),
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

async function release(engine: Engine, table: Map<string, Held>, keys: readonly string[]): Promise<void> {
  for (const key of keys) {
    const entry = table.get(key);
    if (entry === undefined || --entry.holders > 0) continue;
    table.delete(key);
    clearTimeout(entry.timer);
    const { uninstall } = plan(entry.credential, entry.access);
    if (uninstall !== null) await engine.query(uninstall);
    if (entry.lent.size > 0) await engine.drop([...entry.lent.values()]);
  }
}

async function install(engine: Engine, entry: Held): Promise<void> {
  const { install: sql } = plan(entry.credential, entry.access);
  if (sql !== null) {
    await requireHttpfs(engine);
    await engine.query(sql);
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
    const fresh = (await host.credentials(entry.scope, entry.access)).find((c) => c.prefix === prefix);
    if (fresh === undefined) {
      throw new Error(`the host no longer vends ${entry.access} on ${prefix} for ${describe(entry.scope)}`);
    }
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
    // eslint-disable-next-line no-console
    console.error(`[fossil/storage] ${prefix} expired and could not be renewed`, cause);
  }
}

async function requireHttpfs(engine: Engine): Promise<void> {
  if (httpfs.has(engine)) return;
  const rows = await engine.query(
    "SELECT loaded FROM duckdb_extensions() WHERE extension_name = 'httpfs'",
  );
  if (rows[0]?.loaded !== true) {
    throw new Error(
      'the engine has no httpfs loaded: fossil reads s3:// through a scoped secret, and the ' +
        "host's engine loads httpfs before handing it over",
    );
  }
  httpfs.add(engine);
}

export function describe(scope: Scope): string {
  return 'job' in scope ? `job ${scope.job}` : `connection ${scope.connection}`;
}
