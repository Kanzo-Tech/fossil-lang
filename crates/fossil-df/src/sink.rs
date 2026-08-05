//! Native Arrow→Parquet sink — write a [`GraphArData`] to a destination
//! directory in the W0b GraphAr layout (design §A1/§A2):
//!
//! ```text
//! <dest>/graph.graph.yml
//! <dest>/vertex/<Type>.parquet · <Type>.vertex.yml
//! <dest>/edge/<src>_<label>_<dst>/by_source.parquet · by_target.parquet · <dir>.edge.yml
//! ```
//!
//! Native-only: it touches the filesystem. The byte encoding itself lives in
//! [`crate::files`] (wasm-clean, shared with the browser executor) — this module
//! just writes those bytes to a directory, so it is gated off wasm.

use std::fs;
use std::path::Path;

use crate::files::EncodeError;
use crate::GraphArData;

/// Errors writing the GraphAr dataset to disk: filesystem failures, or the
/// shared byte-[`EncodeError`] (Parquet / manifest YAML).
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

impl GraphArData {
    /// Write the whole graph under `dest` (a local directory): one Parquet per
    /// vertex type, the CSR/CSC Parquet pair per edge type, and the three
    /// manifest YAMLs. Creates intermediate directories as needed.
    ///
    /// Encodes via the shared [`Self::to_files`] (the same bytes the browser
    /// host PUTs), then drops each file to disk — the only native-specific step.
    ///
    /// # Errors
    /// Filesystem or Parquet-encode failures, or manifest serialization.
    pub fn write_to_dir(&self, dest: &Path) -> Result<(), SinkError> {
        // One file at a time: encoded, written, dropped. `to_files` would hold
        // every Parquet buffer at once for a list nothing on this path reads.
        // Measured at ten million, it is worth 0.02 GB of peak — noise. The
        // reason it stays is that the list has no reader here, not the number.
        let mut probe = fossil_base::probe::Probe::new("write_to_dir");
        let mut count = 0usize;
        self.try_for_each_file::<SinkError>(|file| {
            let path = dest.join(&file.rel_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, file.bytes)?;
            count += 1;
            Ok(())
        })?;
        probe.mark(&format!("encode + write {count} file(s)"));
        probe.finish();
        Ok(())
    }
}
