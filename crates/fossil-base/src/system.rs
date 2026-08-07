//! Host-injected capabilities (filesystem, time, future: registry, descriptors).
//!
//! Per ADR-0003, descriptors and registry live behind a single `dyn System`
//! indirection on the `Db` trait. Phase 1 only needs `read_file` and `now`;
//! Phase 3+ extends this trait with `input_descriptor`, `output_descriptor`,
//! `registry`, `read_dir`, and `random_seed`.
//!
//! Phase 13 (v0.2, ADR-0037) adds `inferred_descriptor` +
//! `register_inferred_descriptor`: hosts (browser-side `DuckDB-WASM` via
//! `FossilPlayground::register_inferred_descriptor`; native CLI via the
//! `duckdb` crate) populate runtime-introspected column lists BEFORE invoking
//! `compile()`. The Rust compiler never initiates network IO from the
//! WASM-gated crate set — descriptors are HOST-PROVIDED and consumed inside
//! tracked queries via this `System` accessor (mirrors the `read_file`
//! pattern; Salsa-safe; `MAX_PER_MAPPING_FAN_OUT = 1` invariant preserved
//! — see `crates/fossil-hir/tests/invalidation_regression.rs`).

use std::path::Path;
use std::time::SystemTime;

use fossil_descriptors_input::InferredDescriptor;

pub trait System: Send + Sync + std::fmt::Debug {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError>;
    fn now(&self) -> SystemTime;

    /// Look up an [`InferredDescriptor`] by source binding name (e.g. `"users"`).
    ///
    /// Hosts populate the inferred-descriptor table BEFORE invoking any tracked
    /// query that may need it (browser-side: via
    /// `FossilPlayground::register_inferred_descriptor`; native CLI: via the
    /// `duckdb` crate after parsing the source).
    ///
    /// Returns `None` if no descriptor was registered for this source name —
    /// the consumer (`fossil-hir::infer::resolve_source_row`) falls back to
    /// the deprecated CSVW path (when explicit `schema = "..."` arg is
    /// present) or returns `None` (no forward propagation, same as Phase 2
    /// walking-skeleton behaviour).
    ///
    /// OWNED return signature (cloned-on-read). Rationale (LOCKED per plan
    /// 13-02 `<interfaces>` block — not a mid-task decision): the underlying
    /// `Mutex<HashMap>` on [`NativeSystem`] cannot lend a borrow across the
    /// lock guard's lifetime; [`InferredDescriptor`] is small (Vec of
    /// SmolStr-pairs, typically < 50 columns), so clone-on-read is acceptable.
    /// The cost is per-mapping during typecheck, not per-Salsa-query.
    ///
    /// Default impl returns `None` so existing test fixtures + mock Systems
    /// compile unchanged. [`NativeSystem`] overrides to return entries from
    /// its internal `HashMap`.
    ///
    /// Salsa-safe: reads through this method do NOT trigger Salsa
    /// invalidation. The descriptor is host-provided, never produced by a
    /// tracked query. Mirrors `read_file` (ADR-0020); the
    /// `MAX_PER_MAPPING_FAN_OUT = 1` invariant from Phase 2 SC#2 is preserved
    /// (verified by `crates/fossil-hir/tests/invalidation_regression.rs`).
    fn inferred_descriptor(&self, source_name: &str) -> Option<InferredDescriptor> {
        let _ = source_name;
        None
    }

    /// Register an [`InferredDescriptor`] for a source name. Idempotent:
    /// re-registering with the same `source_name` OVERWRITES the previous
    /// entry (intentional — the host may re-introspect when file content
    /// changes).
    ///
    /// Default impl panics — only Systems that opt into inferred-descriptor
    /// storage need to implement this. [`NativeSystem`] does; ad-hoc test
    /// `System` mocks do not unless their tests require it.
    fn register_inferred_descriptor(&self, descriptor: InferredDescriptor) {
        let _ = descriptor;
        panic!("register_inferred_descriptor not implemented for this System");
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
    /// Inferred-descriptor table, keyed by source binding name (e.g. `"users"`
    /// from `users := io.csv(...)`). Populated by the native CLI (plan 13-04a)
    /// BEFORE invoking `typecheck`. Mutex needed for interior mutability —
    /// `register_inferred_descriptor` takes `&self` (the `System` trait method
    /// signature is shared with the WASM host, where workspace mutation goes
    /// through `&self` on the Salsa Db handle).
    inferred: std::sync::Mutex<std::collections::HashMap<smol_str::SmolStr, InferredDescriptor>>,
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

    fn inferred_descriptor(&self, source_name: &str) -> Option<InferredDescriptor> {
        // Lock can only fail if poisoned (another thread panicked while
        // holding it). Treat that as "no descriptor available" rather than
        // propagating the poison — the typecheck fallback path handles
        // missing descriptors gracefully.
        self.inferred.lock().ok()?.get(source_name).cloned()
    }

    fn register_inferred_descriptor(&self, descriptor: InferredDescriptor) {
        let mut lock = self.inferred.lock().expect("mutex poisoned");
        lock.insert(descriptor.source_name.clone(), descriptor);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod inferred_tests {
    use super::*;
    use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
    use fossil_graph_schema::Primitive;

    fn sample(source: &str) -> InferredDescriptor {
        InferredDescriptor {
            source_name: source.into(),
            columns: vec![InferredColumn {
                name: "id".into(),
                primitive: Primitive::Integer,
            }],
            content_hash: String::new(),
        }
    }

    #[test]
    fn native_system_register_then_lookup_returns_same_descriptor() {
        let s = NativeSystem::default();
        s.register_inferred_descriptor(sample("users"));
        let got = s.inferred_descriptor("users").expect("present");
        assert_eq!(got.source_name.as_str(), "users");
        assert_eq!(got.columns.len(), 1);
    }

    #[test]
    fn native_system_lookup_unknown_source_returns_none() {
        let s = NativeSystem::default();
        assert!(s.inferred_descriptor("nope").is_none());
    }

    #[test]
    fn native_system_re_register_overwrites() {
        let s = NativeSystem::default();
        s.register_inferred_descriptor(sample("users"));
        let mut second = sample("users");
        second.columns.push(InferredColumn {
            name: "name".into(),
            primitive: Primitive::String,
        });
        s.register_inferred_descriptor(second);
        let got = s.inferred_descriptor("users").expect("present");
        assert_eq!(got.columns.len(), 2);
    }
}
