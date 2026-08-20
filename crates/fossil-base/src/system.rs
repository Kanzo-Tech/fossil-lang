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

    /// **The provider registry** — every row a program may name after `io.`,
    /// each declaring the extensions it accepts and the capabilities it has.
    ///
    /// This used to be `shape_decoders`, half of the registry, holding only the
    /// rows that read TYPES while the rows that read ROWS lived in a second
    /// table in `fossil-hir` that dispatched by a different criterion. Ruling 13
    /// of `SURFACE-PLAN.md` collapses the two; [`crate::providers`] carries the
    /// argument.
    ///
    /// **Ambient in the context, never part of a query key.** A table of `fn`
    /// and not a trait object: an extension point that has to be named inside a
    /// query is a table of functions, because a trait object has no identity a
    /// query key can hold. That is also what puts a provider on the opposite
    /// side from [`Self::descriptors`] — a descriptor cache is a table of data,
    /// a provider is a table of behaviour. [`Provider`] carries the `&'static` +
    /// `ptr::eq`/`ptr::hash` identity that naming-inside-a-query requires.
    ///
    /// The default is [`DATA`] — the four rows that read data — and it is a real
    /// answer, not a stub: a host that decodes no shape document still has to
    /// recognise `io.csv`. Backward checking with no expected types is correct
    /// rather than degraded, because an undeclared predicate is legal in an open
    /// world. It is NOT the case that a program naming no shape document simply
    /// has no output contract — that was the older rule; naming a document is
    /// mandatory now, a bare property key takes its name from a predicate the
    /// document declares, and a program with none is refused by the checker.
    /// A host that COMPILES installs
    /// `fossil_descriptors_output::PROVIDERS`, which adds the rows carrying a
    /// `fn` into a schema language the compiler may not link.
    ///
    /// Unlike [`Self::descriptors`], the *document* a row decodes IS tracked:
    /// [`crate::shape_documents::decode_shape_document`] takes a
    /// [`crate::files::SourceFile`] input, so editing the document re-runs the
    /// decode and everything downstream. Only the table itself is host-owned and
    /// untracked, and being `&'static` it cannot change within a session.
    ///
    /// See `crates/fossil-base/src/shape_documents.rs` for the rest of the
    /// argument, including the two records where the extension-trait alternative
    /// was tried and did not reach.
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

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default)]
pub struct NativeSystem {
    /// The introspected-schema table this host owns. One field, no methods —
    /// the storage, the locking and the freshness rule all live on
    /// [`DescriptorCache`].
    descriptors: DescriptorCache,
}

#[cfg(not(target_arch = "wasm32"))]
impl System for NativeSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound(path.display().to_string()),
            _ => FsError::Io(e.to_string()),
        })
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn descriptors(&self) -> Option<&DescriptorCache> {
        Some(&self.descriptors)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod inferred_tests {
    use super::*;
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
