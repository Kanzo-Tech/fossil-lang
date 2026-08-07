//! Host-injected capabilities (filesystem, time, future: registry, descriptors).
//!
//! Per ADR-0003, descriptors and registry live behind a single `dyn System`
//! indirection on the `Db` trait. Phase 1 only needs `read_file` and `now`;
//! Phase 3+ extends this trait with `input_descriptor`, `output_descriptor`,
//! `registry`, `read_dir`, and `random_seed`.
//!
//! Phase 13 (v0.2, ADR-0037) added the inferred-descriptor table; ADR-0050
//! moved it behind ONE accessor, [`System::descriptors`]. Hosts (browser-side
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
    /// panics, because there is no longer a default to write — a host either
    /// has a table or answers `None` (ADR-0050, applying ADR-0046 §5: ambient
    /// in the context, never part of a query key).
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

    // Phase 3+ extension points (do not add now — keep the trait surface
    // tight until a downstream consumer forces it):
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
    /// [`DescriptorCache`] (ADR-0050).
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
