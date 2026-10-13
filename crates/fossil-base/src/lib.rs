//! `fossil-base` — Salsa `Db` trait + `System` abstraction.
//!
//! The substrate. `cargo tree -i fossil-base` is the list of what consumes it,
//! and there is deliberately no list here: a written list goes stale the day a
//! crate is renamed and nothing fails.
//!
//! **What a host writes is a Salsa input**, reached through a handle on `Db`:
//! files, sources, connections and the provider catalogue. `System` keeps only what no
//! answer depends on (`/docs/design/incremental`).
//!
//! [`location`] is the rule turning a written reference into a location a
//! reader can open, because that is the file abstraction.
//!
//! **What is deliberately not here**, because a substrate decides nothing about
//! a program: the query that decodes a shape document, which is
//! `fossil_hir::shape_documents` — `providers` still declares the ROW a decoder
//! fills, because a row is a table entry and not a decision.

pub mod connections;
pub mod db;
pub mod diagnostic;
pub mod error;
pub mod files;
pub mod location;
pub mod providers;
pub mod sources;
pub mod system;
/// A shape decoder with no schema language behind it, plus the host that
/// installs it — the seam every crate above this one needs to test against a
/// resolved shape, and which three of them had each written for themselves.
///
/// Off unless the `test-support` feature is on (and always on for this crate's
/// own tests, which is what saves a self-referential dev-dependency).
#[cfg(all(any(feature = "test-support", test), not(target_arch = "wasm32")))]
pub mod test_support;

pub use connections::{Connections, connections, set_connections};
pub use db::{Db, FossilDb};
pub use diagnostic::{Diagnostic, SpanFrame, SpanLabel};
pub use error::{Diagnostics, ErrorGuaranteed};
pub use files::{FileRegistry, Files, SourceFile, file_at, register_document, register_file};
pub use providers::{
    CONSTRUCTOR_PREFIX, Capability, Catalogue, NativeReader, Provider, Registry, RowReader,
    claimed, install, installed, provider,
};
pub use sources::{
    Described, SourceRegistry, Sources, columns, columns_from_json, describe, describe_columns,
    described, refusal_from_json,
};
pub use system::{FsError, System};

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::NativeSystem;
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
