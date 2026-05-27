/**
 * Permalink module barrel (PLAY-04).
 *
 * Public API for the schema-versioned URL-fragment permalink:
 *   - `encode(state)` — JSON → gzip → base64url
 *   - `decode(string)` — inverse, with forward-migration via the registry
 *   - `SCHEMA_VERSION` — current envelope version (currently 1)
 *   - `MAX_PERMALINK_BYTES` — soft cap; `encode` throws above it
 *   - `PermalinkTooLargeError` — thrown by encode above the cap
 *   - `migrate` — the forward-migration helper (exposed for tests + future
 *     server-side validators)
 *   - `PermalinkStateV1` — current state shape
 *
 * Consumers re-import these via `@fossil-lang/playground` (see ../index.tsx
 * for the aliased public re-exports).
 */

export { encode, PermalinkTooLargeError, MAX_PERMALINK_BYTES } from './encode.js';
export { decode } from './decode.js';
export { SCHEMA_VERSION, migrate } from './envelope.js';
// v0.2 active state shape + v0.1 historical shape kept for migration-aware tests.
export type {
  PermalinkState,
  PermalinkStateV1,
  PermalinkStateV2,
} from './envelope.js';
