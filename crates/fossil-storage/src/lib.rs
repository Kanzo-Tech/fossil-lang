//! `fossil-storage` — what fossil does with a storage credential.
//!
//! A host never hands fossil a signed URL. It vends a [`StorageCredential`] —
//! Iceberg REST's `StorageCredential`, verbatim — scoped to one prefix and
//! short-lived, and fossil turns it into access:
//!
//! - a reader over `DuckDB` gets `CREATE OR REPLACE SECRET … SCOPE '<prefix>'`
//!   ([`Grant::install_sql`]), which is what `DuckDB`'s iceberg extension does
//!   with vended credentials. Renewing is the same statement under the same
//!   name, so a view over the prefix never reopens;
//! - an Azure prefix has no `DuckDB` extension in the browser, so a reader
//!   names each file by a scheme-less name ([`Grant::name`]) that the engine
//!   lends to a SAS URL ([`Grant::lend`]) — no glob, a manifest enumerates;
//! - `DataFusion`, and anything that wants bytes rather than SQL, reaches it
//!   through an `object_store` store ([`Storage`], behind the `object-store`
//!   feature), which signs every request itself and renews the credential
//!   before it expires.
//!
//! Without `object-store` the crate is sans-IO. The store enforces the prefix;
//! nothing here can widen a credential.

/// How long a host may take to answer — `/docs/design/failure`'s G1 table.
/// `@fossil-lang/types` holds every host wait to it.
pub const HOST_MS: u64 = 30_000;

/// Renew this long before a credential expires — Iceberg's
/// `VendedCredentialsProvider` margin, and `@fossil-lang/storage`'s.
pub const RENEW_BEFORE_MS: u64 = 5 * 60_000;

pub mod credential;
#[cfg(feature = "js")]
mod js;
#[cfg(feature = "object-store")]
mod store;

pub use credential::{
    Access, Grant, GrantPlan, LocatorName, StorageCredential, StorageError, covering,
};
#[cfg(feature = "js")]
pub use js::JsHost;
#[cfg(feature = "object-store")]
pub use store::{Host, HostError, Scope, Storage};
