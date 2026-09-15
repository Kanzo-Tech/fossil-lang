//! Native sink — write a [`GraphArData`]'s **manifests** to a destination
//! directory:
//!
//! ```text
//! <dest>/graph.graph.yml
//! <dest>/vertex/<Type>.vertex.yml
//! <dest>/edge/<src>_<label>_<dst>/<dir>.edge.yml
//! ```
//!
//! The payload those manifests describe is written by `fossil_layout`'s pass,
//! directly as tiles, out of the same batches — so this module writes no Parquet
//! at all. It wrote one per vertex type and a pair per edge; the pass read every
//! one of them back and deleted it. See [`crate::files`] for the measurement.
//!
//! Native-only: it touches the filesystem. The serialization itself lives in
//! [`crate::files`] (wasm-clean, shared with the browser executor) — this module
//! just writes those bytes to a directory, so it is gated off wasm.

use std::fs;
use std::path::Path;

use crate::GraphArData;
use crate::files::EncodeError;

/// Errors writing the dataset's manifests to disk: filesystem failures, or the
/// shared byte-[`EncodeError`] (manifest YAML).
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

impl GraphArData {
    /// Write the graph's manifests under `dest` (a local directory). Creates
    /// intermediate directories as needed, including the ones the layout pass
    /// then fills.
    ///
    /// Serializes via the shared [`Self::try_for_each_manifest`] (the same bytes
    /// the browser host PUTs), then drops each file to disk — the only
    /// native-specific step.
    ///
    /// # Errors
    /// Filesystem failures, or manifest serialization.
    pub fn write_manifests(&self, dest: &Path) -> Result<(), SinkError> {
        // One file at a time: serialized, written, dropped.
        let mut probe = fossil_mem_probe::Probe::new("write_manifests");
        let mut count = 0usize;
        self.try_for_each_manifest::<SinkError>(|file| {
            let path = dest.join(&file.rel_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, file.bytes)?;
            count += 1;
            Ok(())
        })?;
        probe.mark(&format!("serialize + write {count} manifest(s)"));
        probe.finish();
        Ok(())
    }
}
