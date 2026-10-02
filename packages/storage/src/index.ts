/**
 * @fossil-lang/storage — how every `@fossil-lang/*` package reaches storage, from credentials the
 * host vends (`Host.credentials`) and never from a URL the host signed.
 *
 * One door per capability:
 *
 * - {@link mount} — SQL over a scope through the page's engine: a scoped DuckDB secret per S3
 *   prefix, renewed before it expires, or an Azure file lent by name.
 * - {@link read} — bytes.
 * - {@link resolveDocuments} — the loop that reads what a program names over {@link read}.
 *
 * The translation from a credential to a statement, a name or a request is `fossil-storage`, in
 * WASM — the same Rust a native host renders its secrets with, and the same `object_store` stores
 * `DataFusion` reads through.
 */
// The renewal's two figures, for a host sizing what it vends: a credential that lives less than
// `RENEW_BEFORE_MS` is renewed as soon as it is mounted.
export { mount, RENEW_BEFORE_MS, RETRY_MS, type Mount } from './mount.js';
export { read, type ReadResult, type Target } from './objects.js';
export { resolveDocuments } from './documents.js';
export { initStorage, type InitInput } from './wasm.js';
