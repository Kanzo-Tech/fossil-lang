//! Host-injected capabilities (filesystem, time, future: registry, descriptors).
//!
//! Per ADR-0003, descriptors and registry live behind a single `dyn System`
//! indirection on the `Db` trait. Phase 1 only needs `read_file` and `now`;
//! Phase 3+ extends this trait with `input_descriptor`, `output_descriptor`,
//! `registry`, `read_dir`, and `random_seed`.

use std::path::Path;
use std::time::SystemTime;

pub trait System: Send + Sync + std::fmt::Debug {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError>;
    fn now(&self) -> SystemTime;
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
pub struct NativeSystem;

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
}
