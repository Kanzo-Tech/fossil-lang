//! `fossil-base` — Salsa `Db` trait + `System` abstraction (per ADR-0003).
//!
//! This crate is the substrate every downstream compiler crate
//! (`fossil-syntax`, `fossil-hir`, `fossil-mir`, `fossil-codegen`,
//! `fossil-runtime`, `fossil-cli`, `fossil-lsp`, `fossil-wasm`) consumes.
//!
//! **Public-API commitment** to Phase 2-9: the signatures here are stable.
//! Changes require an ADR superseding ADR-0003.

pub mod db;
pub mod diagnostic;
pub mod error;
pub mod files;
pub mod probe;
pub mod system;

pub use db::{Db, FossilDb};
pub use diagnostic::{Diagnostic, Severity, Span};
pub use error::{ErrorGuaranteed, bug, delay_span_bug};
pub use files::{Files, SourceFile};
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
