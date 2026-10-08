// The glue is imported from this leaf module only, never from the entry — see
// `@fossil-lang/executor`'s `client.ts` for why a `sideEffects: false` bundler must not see it twice.
import init, {
  storageCovering,
  storageGrant,
  storageName,
  storageRead as rawRead,
  type InitInput,
} from '../pkg/fossil_storage_wasm.js';
import { FossilError, isFossilError, type Access, type Host, type StorageCredential } from '@fossil-lang/types';
import { loader, type GrantPlan, type LocationName } from '@fossil-lang/types/internal';

export type { InitInput };

/**
 * Boot the `fossil-storage` module — for Node, with the bytes, and nothing else. Every door boots it
 * itself, and the glue finds its `.wasm` through `new URL(…, import.meta.url)`, which a bundler
 * emits as an asset.
 *
 * A boot that succeeded is kept; one that failed is not, so the next call tries again.
 *
 * @throws {FossilError} `module/unreachable` when the module could not be fetched within 60 s,
 *   `internal/bug` when it would not instantiate.
 */
export const initFossilStorage = loader<InitInput>('fossil_storage_wasm_bg.wasm', init);

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

export const nameOf = (credential: StorageCredential, location: string): LocationName =>
  guarded('fossil-storage failed naming a location', () => storageName(credential, location));

/** The credential whose prefix covers `location`, by `fossil_storage::covering`. */
export function covering(
  credentials: readonly StorageCredential[],
  location: string,
): StorageCredential | undefined {
  const prefix = storageCovering(credentials.map((c) => c.prefix), location);
  return credentials.find((c) => c.prefix === prefix);
}
