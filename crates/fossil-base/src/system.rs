//! Host-injected capabilities: a filesystem and a clock, and the provider rows
//! the host declares.
//!
//! Nothing a compile's answer depends on lives here. What a host writes — files,
//! what each source is, the installed rows — is a Salsa input with a handle on
//! `Db` (`/docs/design/incremental`); a table read beside Salsa is memoised with
//! the answer and never invalidated, which is what the descriptor table that
//! used to be here did.

use std::path::Path;
use std::time::SystemTime;

use crate::providers::{DATA, Provider};

pub trait System: Send + Sync + std::fmt::Debug {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError>;
    fn now(&self) -> SystemTime;

    /// **What this host declares it recognises**, and nothing reads it directly:
    /// `FossilDb::new` copies it into [`crate::providers::Registry`], a Salsa
    /// input, and every query goes through [`crate::providers::installed`].
    ///
    /// The rows stay `&'static` because a row's identity is its address, and a
    /// `Vec<&'static Provider>` inside an input is exactly that. A table read
    /// from a file at run time cannot be `&'static`, which is why the read path
    /// moved to the input and this method stayed a declaration.
    ///
    /// **This method is a seam that should close.** All but two implementations
    /// return the identical `fossil_hir::providers::PROVIDERS`, so "the host
    /// chooses what is installed" is a choice nobody makes. When the catalogue
    /// is loaded from a file there is one loader, and they all go with it.
    ///
    /// The default is [`DATA`] — the four rows that read data — and it is a real
    /// answer, not a stub: a host that decodes no shape document still has to
    /// recognise `io.csv`.
    fn providers(&self) -> &'static [&'static Provider] {
        DATA
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum FsError {
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(String),
}

// `NativeSystem` — a filesystem, a clock and the DEFAULT provider table — lived
// here beside the trait it implements, and is `crate::test_support` now.
//
// It has no production consumer, and that is not an accident: a host that
// COMPILES a program has to install the rows that read types, and the trait
// default is the data rows alone, so the real host (the wasm workspace's)
// declares its own. What is left is a fixture: the cheapest `System` a test can
// stand up.
//
// A fixture on the crate root is a fixture something will eventually reach for
// in production, and «in `base` only traits» is the rule that says so.
