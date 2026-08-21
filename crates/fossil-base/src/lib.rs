//! `fossil-base` — Salsa `Db` trait + `System` abstraction.
//!
//! This crate is the substrate every downstream compiler crate
//! (`fossil-syntax`, `fossil-hir`, `fossil-mir`, `fossil-codegen`,
//! `fossil-runtime`, `fossil-cli`, `fossil-lsp`, `fossil-wasm`) consumes.
//!
//! **Public-API commitment**: the signatures here are stable. The
//! change this commitment exists to prevent is widening the `Db` trait —
//! descriptors, registry and host capabilities go behind `System`, never onto
//! `Db`, and every crate above this one is built on that being true.

pub mod db;
pub mod diagnostic;
pub mod error;
pub mod files;
pub mod locator;
pub mod providers;
pub mod shape_documents;
pub mod system;
/// A shape decoder with no schema language behind it, plus the host that
/// installs it — the seam every crate above this one needs to test against a
/// resolved shape, and which three of them had each written for themselves.
///
/// Off unless the `test-support` feature is on (and always on for this crate's
/// own tests, which is what saves a self-referential dev-dependency).
#[cfg(all(any(feature = "test-support", test), not(target_arch = "wasm32")))]
pub mod test_support;

pub use db::{Db, FossilDb};
pub use diagnostic::{Diagnostic, Severity, Span, SpanFrame, SpanLabel};
pub use error::{ErrorGuaranteed, bug, delay_span_bug, raise};
pub use files::{FileRegistry, Files, SourceFile, file_at, register_file};
pub use locator::{SourceAnchor, program_dir};
pub use providers::{
    Capability, Catalogue, NativeReader, Provider, Registry, RowReader, install, installed,
    provider,
};
pub use shape_documents::{TypeDocument, decode_shape_document, shape_document};
pub use system::{FsError, System};

#[cfg(not(target_arch = "wasm32"))]
pub use system::NativeSystem;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn db_can_be_constructed_and_query_sourcefile() {
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, "hello".to_string(), "test.fossil".to_string());
        assert_eq!(file.text(&db), "hello");
        assert_eq!(file.path(&db), "test.fossil");
    }
}
