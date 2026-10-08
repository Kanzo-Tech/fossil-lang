/**
 * @fossil-lang/storage — how every `@fossil-lang/*` package reaches storage, from credentials the
 * host vends (`Host.credentials`) and never from a URL the host signed.
 *
 * - {@link mount} — SQL over a scope through the page's engine: a scoped DuckDB secret per S3
 *   prefix, renewed before it expires, or an Azure file registered by name.
 * - {@link resolveDocuments} — the loop that reads what a program names, through `object_store`.
 *
 * The translation from a credential to a statement, a name or a request is `fossil-storage`, in
 * WASM — the same Rust a native host renders its secrets with, and the same `object_store` stores
 * `DataFusion` reads through. Its siblings are its callers; a host seldom is.
 */
export { mount, type Mount } from './mount.js';
export { resolveDocuments } from './documents.js';
export { initFossilStorage, type InitInput } from './wasm.js';
