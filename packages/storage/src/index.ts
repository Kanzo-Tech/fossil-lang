/**
 * @fossil-lang/storage — how every `@fossil-lang/*` package reaches storage, from credentials the
 * host vends (`Host.credentials`) and never from a URL the host signed.
 *
 * One door per capability:
 *
 * - {@link mount} — SQL over a scope through the page's engine: a scoped DuckDB secret per S3
 *   prefix, renewed before it expires, or an Azure file lent by name.
 * - {@link read} — bytes, by a signed GET.
 * - {@link write} — bytes under the one prefix a scope vends `write` on, by a signed PUT.
 * - {@link resolveDocuments} — the loop that reads what a program names over {@link read}.
 *
 * The translation from a credential to a statement, a name or a signature is `fossil-storage`,
 * in WASM — the same Rust a native host renders its secrets with.
 */
export { mount, type Mount } from './mount.js';
export { read, write, type ReadResult, type Target } from './objects.js';
export { resolveDocuments } from './documents.js';
export { initStorage, type InitInput } from './wasm.js';
