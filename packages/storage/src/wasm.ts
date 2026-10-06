// The glue is imported from this leaf module only, never from the entry — see
// `@fossil-lang/executor`'s `client.ts` for why a `sideEffects: false` bundler must not see it twice.
import init, {
  storageCovering,
  storageGrant,
  storageName,
  storageRead as rawRead,
  type InitInput,
} from '../pkg/fossil_storage_wasm.js';
import {
  FossilError,
  isFossilError,
  loader,
  type Access,
  type GrantPlan,
  type Host,
  type LocatorName,
  type StorageCredential,
} from '@fossil-lang/types';

export type { InitInput };

/**
 * Boot the `fossil-storage` module. Every door awaits it with nothing, and the glue finds its
 * `.wasm` through `new URL(…, import.meta.url)`, which a bundler emits as an asset; a host with no
 * bundler (Node) calls it first with the bytes.
 *
 * A boot that succeeded is kept; one that failed is not, so the next call tries again.
 *
 * @throws {FossilError} `module/unreachable` when the module could not be fetched within 60 s,
 *   `internal/bug` when it would not instantiate.
 */
export const initStorage = loader<InitInput>('fossil_storage_wasm_bg.wasm', init);

/**
 * `call`, with a failure fossil did not raise — a panic is a `RuntimeError: unreachable`, and the
 * panic hook has printed its message — reported as `internal/bug`, the original kept as its cause.
 */
function guarded<T>(what: string, call: () => T): T {
  try {
    return call();
  } catch (cause) {
    throw isFossilError(cause) ? cause : bug(what, cause);
  }
}

function bug(what: string, cause: unknown): FossilError<'internal/bug'> {
  return FossilError.of('internal/bug', { what }, { cause });
}

export async function storageRead(host: Host, targets: readonly unknown[]): Promise<unknown> {
  try {
    return await rawRead(host, targets);
  } catch (cause) {
    throw isFossilError(cause) ? cause : bug('fossil-storage failed reading', cause);
  }
}

export const plan = (credential: StorageCredential, access: Access): GrantPlan =>
  guarded('fossil-storage failed planning a grant', () => storageGrant(credential, access));

export const nameOf = (credential: StorageCredential, locator: string): LocatorName =>
  guarded('fossil-storage failed naming a locator', () => storageName(credential, locator));

/** The credential whose prefix covers `locator`, by `fossil_storage::covering`. */
export function covering(
  credentials: readonly StorageCredential[],
  locator: string,
): StorageCredential | undefined {
  const prefix = storageCovering(credentials.map((c) => c.prefix), locator);
  return credentials.find((c) => c.prefix === prefix);
}
