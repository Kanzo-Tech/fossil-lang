# @fossil-lang/storage

How every `@fossil-lang/*` package reaches storage: from credentials the host vends, never from a
URL the host signed.

A host implements `Host` from `@fossil-lang/types` — `connections()` and
`credentials(scope, access)`, answering Iceberg REST's `StorageCredential[]` — and hands it to
`@fossil-lang/corpus`, `@fossil-lang/introspect`, `@fossil-lang/executor` or `openProgram`. They
come here; a host seldom does.

Each call is handed `{ signal }`, which aborts when fossil stops waiting. A host has 30 s to
answer; one that does not is `storage/host-silent`, from this package and from the Rust half alike.

| door | for | how |
| --- | --- | --- |
| `mount(engine, host, scope, access, { signal })` | SQL through the page's engine | `CREATE OR REPLACE SECRET … SCOPE '<prefix>'` per S3 prefix, renewed at `expires − 5 min` under the same name, shared by every mount of the prefix and dropped by the last; an Azure file is registered by name to its SAS URL (no Azure extension in DuckDB-WASM, so no glob) |
| `resolveDocuments(workspace, host, { signal })` | the documents a program names | an `object_store` GET per file, one `credentials` call per connection, until nothing new is missing |

The engine must have `httpfs` loaded; `mount` says so when it has not. The translation from a
credential to a statement, a name or a store is the Rust crate `fossil-storage`, compiled to
`pkg/` — the same code a native host renders its secrets with, and the same stores
`@fossil-lang/executor`'s DataFusion reads through. The store must expose `ETag` to CORS for a
multipart write. In Node, call
`initFossilStorage(bytes)` first; a bundler finds the `.wasm` on its own. A `Mount` is released
with `close()`, or by `await using`.
