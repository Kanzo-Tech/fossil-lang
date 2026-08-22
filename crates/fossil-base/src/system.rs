//! Host-injected capabilities (filesystem, time, future: registry, descriptors).
//!
//! **The `Db` trait stays thin, and descriptors and registry live behind a
//! single `dyn System` indirection on it** — so a new host capability widens one
//! trait and never the query surface. Today that is `read_file` and `now`, plus
//! the two tables below; `read_dir`, `random_seed` and the function registry are
//! extension points, listed at the foot of the trait and not written yet.
//!
//! The inferred-descriptor table arrived as a read/write method pair that three
//! `System` impls each re-implemented, and it sits behind ONE accessor now,
//! [`System::descriptors`]. Hosts (browser-side
//! `DuckDB-WASM` via `FossilPlayground::registerInferredDescriptor`; the
//! native engine via the `duckdb` crate) populate runtime-introspected column
//! lists BEFORE invoking `compile()`. The Rust compiler never initiates
//! network IO from the WASM-gated crate set — descriptors are HOST-PROVIDED
//! and consumed inside tracked queries through this accessor (mirrors the
//! `read_file` pattern; Salsa-safe; `MAX_PER_MAPPING_FAN_OUT = 1` invariant
//! preserved — see `crates/fossil-hir/tests/invalidation_regression.rs`).

use std::path::Path;
use std::time::SystemTime;

use fossil_descriptors_input::DescriptorCache;

use crate::providers::{DATA, Provider};

pub trait System: Send + Sync + std::fmt::Debug {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError>;
    fn now(&self) -> SystemTime;

    /// The host's [`DescriptorCache`] — the introspected input schemas, keyed
    /// by source URI — or `None` for a host that does not keep one.
    ///
    /// This is the whole descriptor surface: one accessor handing out a
    /// reference to a plain table, in place of the read/write method pair that
    /// three `System` impls each re-implemented over a private
    /// `Mutex<HashMap>`. The registration side no longer has a default that
    /// panics, because there is no longer a default to write — a host either has
    /// a table or answers `None`. **A host capability is ambient in the context
    /// and never part of a query key**, which is what lets this be an accessor
    /// at all.
    ///
    /// It is a concrete struct and not a `fn` table because the split is data
    /// against behaviour: [`Self::providers`] is a table of BEHAVIOUR and this
    /// is a table of DATA, and a function pointer that returned descriptors
    /// would be the indirection without the reason for it.
    ///
    /// `None` is a real answer and not a stub: `fossil-df-wasm`'s executor
    /// takes its schemas from the plan it was handed and has nothing to cache.
    ///
    /// Salsa-safe: reading through this accessor does NOT register a Salsa
    /// dependency and does NOT trigger invalidation. The cache is host-owned
    /// state, never produced by a tracked query. A host that wants downstream
    /// queries to re-run bumps the source file's text via `set_text`, which
    /// Salsa already tracks.
    fn descriptors(&self) -> Option<&DescriptorCache> {
        None
    }

    /// **What this host declares it recognises.** It is no longer what anything
    /// reads: `FossilDb::new` copies it into [`crate::providers::Registry`], a
    /// Salsa input, and every query goes through
    /// [`crate::providers::installed`].
    ///
    /// The old signature *was* the read path, and its `&'static` is what put
    /// ruling 14 out of reach — a table that has to outlive the program cannot
    /// be read from a file, and a table that is not an input cannot invalidate
    /// anything downstream. Both are now possible without this method changing.
    ///
    /// It sat beside [`Self::descriptors`] under the argument that one is a
    /// table of DATA and the other a table of BEHAVIOUR. That distinction is
    /// real and survives. What did not survive is the conclusion drawn from it —
    /// that a table of behaviour must therefore be ambient and untracked. What
    /// it must be is `&'static`, because a row's identity is its address, and a
    /// `Vec<&'static Provider>` inside an input is exactly that.
    ///
    /// **This method is the seam that ruling 14 deletes.** Twelve of the
    /// thirteen implementations return the identical
    /// `fossil_descriptors_output::PROVIDERS`, so "the host chooses what is
    /// installed" is a choice nobody makes. When the catalogue is loaded from a
    /// file there is one loader, and these thirteen go with it.
    ///
    /// The default is [`DATA`] — the four rows that read data — and it is a real
    /// answer, not a stub: a host that decodes no shape document still has to
    /// recognise `io.csv`.
    fn providers(&self) -> &'static [&'static Provider] {
        DATA
    }

    // Extension points considered and deliberately not added — keep the trait
    // surface tight until a downstream consumer forces one:
    //   fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, FsError>;
    //   fn input_descriptor(&self, kind: &str) -> Option<&dyn InputDescriptor>;
    //   fn output_descriptor(&self, kind: &str) -> Option<&dyn OutputDescriptor>;
    //   fn registry(&self) -> &FunctionRegistry;
    //   fn random_seed(&self) -> u64;
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
// It had no production consumer. Measured across the workspace: every mention
// outside this crate is `#[cfg(test)]`, a `tests/` file, a bench or an example,
// and the one in `fossil-lsp` is a docblock saying `LspSystem` replaced it. That
// is not an accident of history — a host that COMPILES a program has to install
// the rows that read types, and the trait default is the data rows alone, so
// every real host (`LspSystem`, the engine's, the playground's) declares its
// own. What was left is a fixture: the cheapest `System` a test can stand up.
//
// A fixture on the crate root is a fixture something will eventually reach for
// in production, and «in `base` only traits» is the rule that says so.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod inferred_tests {
    use super::*;
    use crate::test_support::NativeSystem;
    use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
    use fossil_graph_schema::Primitive;

    fn sample(uri: &str) -> InferredDescriptor {
        InferredDescriptor {
            uri: uri.into(),
            columns: vec![InferredColumn {
                name: "id".into(),
                primitive: Primitive::Integer,
            }],
            freshness_token: "t1".into(),
        }
    }

    #[test]
    fn native_system_register_then_lookup_returns_same_descriptor() {
        let s = NativeSystem::default();
        let cache = s.descriptors().expect("the native host keeps a table");
        cache.insert(sample("examples/users.csv"));
        let got = cache.get("examples/users.csv").expect("present");
        assert_eq!(got.uri.as_str(), "examples/users.csv");
        assert_eq!(got.columns.len(), 1);
    }

    #[test]
    fn native_system_lookup_unknown_uri_returns_none() {
        let s = NativeSystem::default();
        assert!(s.descriptors().expect("table").get("nope.csv").is_none());
    }

    /// The default `System` has no table. It used to have one that panicked
    /// the moment anyone wrote to it; the answer is now a value the caller can
    /// branch on.
    #[test]
    fn a_system_without_a_table_says_so_instead_of_panicking() {
        #[derive(Debug)]
        struct Bare;
        impl System for Bare {
            fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
                Err(FsError::NotFound(path.display().to_string()))
            }
            fn now(&self) -> SystemTime {
                SystemTime::UNIX_EPOCH
            }
        }
        assert!(Bare.descriptors().is_none());
    }
}
