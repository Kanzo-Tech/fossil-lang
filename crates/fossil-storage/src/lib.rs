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
//! - a writer, or a reader that wants bytes rather than SQL, signs one request
//!   ([`Grant::sign`]): `SigV4` for S3, the SAS for Azure.
//!
//! Sans-IO: the clock is an argument, so the crate is the same on wasm32 and
//! native. The store enforces the prefix; nothing here can widen a credential.

pub mod credential;
pub mod resolved;
mod sigv4;

pub use credential::{Access, Grant, SignedRequest, StorageCredential, StorageError};
pub use resolved::{CloudSecret, ResolvedPath};
