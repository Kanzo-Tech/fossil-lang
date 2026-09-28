// The glue is imported from this leaf module only, never from the entry — see
// `@fossil-lang/executor`'s `client.ts` for why a `sideEffects: false` bundler must not see it twice.
import init, {
  storageGrant,
  storageName,
  storageSign,
  type InitInput,
} from '../pkg/fossil_storage_wasm.js';
import type { Access, StorageCredential } from '@fossil-lang/types';

export type { InitInput };

let booted: Promise<unknown> | null = null;

/**
 * Boot the `fossil-storage` module, memoised. Every door awaits it with nothing, and the glue finds
 * its `.wasm` through `new URL(…, import.meta.url)`, which a bundler emits as an asset; a host with
 * no bundler (Node) calls it first with the bytes.
 */
export function initStorage(wasm?: InitInput): Promise<unknown> {
  booted ??= init(wasm === undefined ? undefined : { module_or_path: wasm });
  return booted;
}

export interface GrantPlan {
  prefix: string;
  install: string | null;
  uninstall: string | null;
  expiresAtMs: number | null;
}

export const plan = (credential: StorageCredential, access: Access): GrantPlan =>
  storageGrant(credential, access) as GrantPlan;

export const nameOf = (
  credential: StorageCredential,
  locator: string,
): { name: string; lend: string | null } => storageName(credential, locator);

export const signed = (
  credential: StorageCredential,
  method: 'GET' | 'PUT',
  locator: string,
): { url: string; headers: Record<string, string> } =>
  storageSign(credential, method, locator, Date.now());

/** The credential whose prefix is the longest one covering `locator`. */
export function covering(
  credentials: readonly StorageCredential[],
  locator: string,
): StorageCredential | undefined {
  let best: StorageCredential | undefined;
  for (const c of credentials) {
    if (locator.startsWith(c.prefix) && c.prefix.length > (best?.prefix.length ?? -1)) best = c;
  }
  return best;
}
