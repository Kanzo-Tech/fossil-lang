# @fossil-lang/storage

How every `@fossil-lang/*` package reaches storage: from credentials the host vends, never from a
URL the host signed.

A host implements `Host` from `@fossil-lang/types` — `connections()` and
`credentials(scope, access)`, answering Iceberg REST's `StorageCredential[]` — and hands it to
`@fossil-lang/corpus`, `@fossil-lang/introspect`, `@fossil-lang/executor` or `openProgram`. They
come here; a host seldom does.

| door | for | how |
| --- | --- | --- |
| `mount(engine, host, scope, access)` | SQL through the page's engine | `CREATE OR REPLACE SECRET … SCOPE '<prefix>'` per S3 prefix, renewed at `expires − 5 min` under the same name, shared by every mount of the prefix and dropped by the last; an Azure file is lent by name to its SAS URL (no Azure extension in DuckDB-WASM, so no glob) |
| `read(host, targets)` | bytes | a GET signed with SigV4 (or the SAS) per file, one `credentials` call per connection |
| `write(host, scope, files)` | a job's output | a PUT signed with SigV4 (or the SAS) under the one prefix the scope vends `write` on |
| `resolveDocuments(workspace, host)` | the documents a program names | `read`, until nothing new is missing |

The engine must have `httpfs` loaded; `mount` says so when it has not. The translation from a
credential to a statement, a name or a signature is the Rust crate `fossil-storage`, compiled to
`pkg/` — the same code a native host renders its secrets with. In Node, call
`initStorage(bytes)` first; a bundler finds the `.wasm` on its own.
