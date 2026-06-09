//! Native Arrow→Parquet sink — write a [`GraphArData`] to a destination
//! directory in the W0b GraphAr layout (design §A1/§A2):
//!
//! ```text
//! <dest>/graph.graph.yml
//! <dest>/vertex/<Type>.parquet · <Type>.vertex.yml
//! <dest>/edge/<src>_<label>_<dst>/by_source.parquet · by_target.parquet · <dir>.edge.yml
//! ```
//!
//! Native-only: it touches the filesystem and links the `parquet` `ArrowWriter`.
//! The browser path produces the same `GraphArData` and writes the Parquet bytes
//! with parquet-wasm in JS (design §E), so this module is gated off wasm.

use std::fs::{self, File};
use std::path::Path;

use datafusion::arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;

use crate::GraphArData;

/// Errors writing the GraphAr dataset to disk.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parquet encode: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("manifest yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}

impl GraphArData {
    /// Write the whole graph under `dest` (a local directory): one Parquet per
    /// vertex type, the CSR/CSC Parquet pair per edge type, and the three
    /// manifest YAMLs. Creates intermediate directories as needed.
    ///
    /// # Errors
    /// Filesystem or Parquet-encode failures, or manifest serialization.
    pub fn write_to_dir(&self, dest: &Path) -> Result<(), SinkError> {
        for v in &self.vertices {
            let path = dest.join(format!("vertex/{}.parquet", v.type_name));
            write_parquet(&path, &v.batches)?;
        }
        for e in &self.edges {
            let dir = format!("edge/{}_{}_{}", e.src_type, e.edge_type, e.dst_type);
            write_parquet(&dest.join(format!("{dir}/by_source.parquet")), &e.by_source)?;
            write_parquet(&dest.join(format!("{dir}/by_target.parquet")), &e.by_target)?;
        }
        for manifest in self.manifests()? {
            let path = dest.join(&manifest.rel_path);
            ensure_parent(&path)?;
            fs::write(path, manifest.yaml)?;
        }
        Ok(())
    }
}

/// Write `batches` to a Parquet file at `path` (uncompressed — `parquet`'s
/// default props; the discovery reader decodes any valid Parquet). An empty
/// batch set writes nothing (a 0-row type has no schema to declare here).
fn write_parquet(path: &Path, batches: &[RecordBatch]) -> Result<(), SinkError> {
    let Some(first) = batches.first() else {
        return Ok(());
    };
    ensure_parent(path)?;
    let mut writer = ArrowWriter::try_new(File::create(path)?, first.schema(), None)?;
    for batch in batches {
        writer.write(batch)?;
    }
    writer.close()?;
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<(), SinkError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
