//! `fossil-resolver` — host-injected cloud path resolution.
//!
//! When a `.fossil` mapping references an external source — a CSV at
//! `s3://bucket/users.csv`, a Parquet at `az://container/x.parquet`, or a
//! host-mediated reference like `@conn-name/path` — the runtime needs two
//! things to actually read it:
//!
//! 1. A concrete URL the underlying `DuckDB` reader can dereference.
//! 2. A provider-typed [`CloudSecret`] (account keys, tokens, region) that
//!    `DuckDB` installs via a scoped `CREATE SECRET` before the
//!    `read_parquet(url)` / `COPY … TO url` call.
//!
//! Fossil itself does NOT know about clouds — the [`PathResolver`] trait is
//! the host-injection seam — the same pattern as `fossil-base::System` for the
//! filesystem: the capability is behind a host-supplied object, never on the
//! Salsa `Db` trait. A standalone CLI mounts
//! [`DefaultPathResolver`] which passes paths through unchanged; a
//! multi-tenant host like Keasy mounts an implementation that resolves
//! `@conn-name/path` against the calling org's per-connection credentials.
//!
//! ## Design notes (vs the angelip2303 predecessor)
//!
//! This crate replaces `fossil_lang::traits::resolver` (the angelip2303
//! fork's resolver — see auto-memory
//! `project_fossil_graph_reference_architecture.md`). Three deliberate
//! divergences:
//!
//! - **No Polars.** The predecessor exposed `polars::prelude::PlPath` +
//!   `CloudOptions` because the angelip2303 runtime drove I/O via Polars
//!   `LazyFrame::scan_parquet`. The rmlext runtime drives `DuckDB` directly,
//!   which authenticates via a scoped `CREATE SECRET` (typed per provider —
//!   s3/azure/gcs — and matched to a URL by longest-prefix scope, so distinct
//!   per-connection secrets never collide the way a global `SET` dance would).
//!   Dropping the Polars dep keeps the resolver dependency-light and native.
//! - **`secrecy::SecretString` values.** Cloud config keys
//!   (`azure_storage_account_key`, `aws_access_key_secret`, …) are
//!   secrets. The predecessor used raw `String`, leaking into log frames
//!   any time the map round-tripped through `Debug`. [`SecretString`]
//!   forbids `Debug`/`Display` and zeroes the inner buffer on drop.
//! - **Typed [`ResolveError`].** The predecessor returned `Result<_, String>`
//!   so callers couldn't pattern-match on the failure shape. The new error
//!   enum distinguishes the host-reference-unsupported case (the standalone
//!   resolver's only failure mode) from generic resolver-supplied errors.

// NATIVE-only by intent. Cloud credential handling needs OS-level fetch
// and explicit secret material; the threat model deliberately keeps
// credentials off the wasm boundary. The cfg-tripwire fails the build at
// compile time rather than letting a wasm32 host link a broken
// `DefaultPathResolver` at runtime.
#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-resolver is native-only (cloud credentials must not cross the wasm boundary)"
);

pub mod error;
pub mod resolved;
pub mod resolver;

pub use error::ResolveError;
pub use resolved::{CloudSecret, ResolvedPath};
pub use resolver::{DefaultPathResolver, PathResolver};
