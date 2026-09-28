import type { Host, Scope, StorageCredential } from '@fossil-lang/types';

import { describe } from './mount.js';
import { covering, initStorage, signed } from './wasm.js';

/** A file to read: where it is, and the connection it lies under when it has one. */
export interface Target {
  locator: string;
  connection?: string;
}

export type ReadResult = { ok: true; bytes: Uint8Array } | { ok: false; reason: string };

/**
 * Read each target's bytes, in order, by requests signed with the credentials the host vends.
 *
 * One `credentials` call per connection named, not per file. A target with no connection is read
 * as it is only when it is a public `http(s)` URL — there is nothing to vend for it. A failure is
 * the target's own answer and never the batch's; a reason never carries a signed URL.
 */
export async function read(host: Host, targets: readonly Target[]): Promise<ReadResult[]> {
  await initStorage();
  const vended = new Map<string, Promise<StorageCredential[]>>();
  const credentialsOf = (connection: string): Promise<StorageCredential[]> => {
    let pending = vended.get(connection);
    if (pending === undefined) {
      pending = host.credentials({ connection }, 'read');
      vended.set(connection, pending);
    }
    return pending;
  };
  return Promise.all(
    targets.map(async ({ locator, connection }): Promise<ReadResult> => {
      try {
        let url = locator;
        let headers: Record<string, string> = {};
        if (connection !== undefined) {
          const credential = covering(await credentialsOf(connection), locator);
          if (credential === undefined) {
            return { ok: false, reason: `the host vends nothing covering it for connection ${connection}` };
          }
          ({ url, headers } = signed(credential, 'GET', locator));
        } else if (!/^https?:\/\//.test(locator)) {
          return { ok: false, reason: 'it names no connection and is not a public URL' };
        }
        const res = await fetch(url, { headers });
        if (!res.ok) return { ok: false, reason: `HTTP ${res.status}` };
        return { ok: true, bytes: new Uint8Array(await res.arrayBuffer()) };
      } catch (cause) {
        return { ok: false, reason: cause instanceof Error ? cause.message : String(cause) };
      }
    }),
  );
}

/**
 * Write each file under the one prefix the host vends `write` on for `scope`, by requests signed
 * with that credential, and answer the prefix. A scope that vends several prefixes has no single
 * place to write to, and is refused.
 */
export async function write(
  host: Host,
  scope: Scope,
  files: readonly { path: string; bytes: Uint8Array }[],
): Promise<string> {
  await initStorage();
  const credentials = await host.credentials(scope, 'write');
  if (credentials.length !== 1) {
    throw new Error(
      `the host vended ${credentials.length} write credentials for ${describe(scope)}; a write needs exactly one prefix`,
    );
  }
  const credential = credentials[0]!;
  await Promise.all(
    files.map(async ({ path, bytes }) => {
      const { url, headers } = signed(credential, 'PUT', `${credential.prefix}${path}`);
      const res = await fetch(url, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/octet-stream', ...headers },
        body: bytes as BodyInit,
      });
      if (!res.ok) throw new Error(`write ${credential.prefix}${path}: HTTP ${res.status}`);
    }),
  );
  return credential.prefix;
}
