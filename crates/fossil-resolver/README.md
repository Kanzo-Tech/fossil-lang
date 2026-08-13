# fossil-resolver

Host-injected cloud path resolution. Maps a user-written `.fossil` path string (`s3://bucket/users.csv`, `@conn-name/users.csv`, `file:///tmp/x.csv`) to a concrete URL + provider-specific cloud configuration the runtime threads into DuckDB.

The `PathResolver` trait is the host-injection seam, the same shape `fossil-base::System` has for the filesystem: a host capability goes behind one trait so the query surface never widens to meet it. `fossil-cli` and the playground mount the `DefaultPathResolver` which only handles local + public URLs; multi-tenant hosts like Keasy mount an implementation that resolves `@conn-name` against the calling org's per-connection credentials.

## Quick shape

```rust
let resolver: Arc<dyn PathResolver> = Arc::new(DefaultPathResolver);
let resolved = resolver.resolve("s3://my-bucket/users.csv")?;
// resolved.url()           — physical URL for DuckDB read_parquet
// resolved.cloud_config()  — keys for DuckDB SET, values are SecretString
// resolved.for_each_setting(|k, v| /* SET k=v */ )  — preferred apply path
```

## Design

This crate replaces the angelip2303 `fossil_lang::traits::resolver` module. Three deliberate divergences (see `src/lib.rs` for the full rationale):

- **No Polars.** Plain URL strings + provider key/value pairs, not `polars::prelude::PlPath`.
- **`secrecy::SecretString` values.** Cloud auth values never `Debug`-print and zero on drop.
- **Typed `ResolveError`.** Callers pattern-match on `HostReferenceUnsupported` vs `Host(_)` instead of substring-matching a `String`.

NATIVE-only: the cfg-tripwire in `lib.rs` fails the build at compile time on `wasm32-unknown-unknown` because credentials must not cross the WASM boundary.
