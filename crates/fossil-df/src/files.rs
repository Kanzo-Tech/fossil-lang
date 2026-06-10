//! The GraphAr output as in-memory bytes — the single Arrow→Parquet encoder
//! shared by the native fs sink ([`crate::sink`]) and the browser executor
//! (`fossil-df-wasm`).
//!
//! Universal-substrate decision: ONE parquet encoder in Rust (parquet-rs,
//! native + wasm), no parquet-wasm JS. [`GraphArData::to_files`] turns the
//! materialised graph into the W0b GraphAr tree as `(rel_path, bytes)` pairs —
//! one Parquet per vertex type, the CSR/CSC Parquet pair per edge, and the three
//! manifest YAMLs. The native sink writes each pair to disk; the wasm host hands
//! them to JS for signed `PUT`. Pure in-memory (`Vec<u8>` writer), so the encoder
//! itself never touches the filesystem and compiles to `wasm32`.

use datafusion::arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;

use crate::GraphArData;

/// One output file of the GraphAr dataset: its dataset-relative path
/// (`vertex/<Type>.parquet`, `edge/<dir>/by_source.parquet`, `graph.graph.yml`,
/// …) and its encoded bytes. The path keys both the on-disk layout (native sink)
/// and the signed-`PUT` object key (browser host).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphArFile {
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// Encoding the GraphAr dataset to bytes failed — Parquet encode or manifest
/// YAML serialization. Filesystem errors live in [`crate::sink::SinkError`]
/// (native-only); this stays wasm-clean.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("parquet encode: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("manifest yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}

impl GraphArData {
    /// The whole GraphAr dataset as in-memory `(rel_path, bytes)` files: every
    /// vertex Parquet, every edge CSR/CSC Parquet pair, and the three manifest
    /// YAMLs ([`Self::manifests`]). A zero-row vertex/edge encodes nothing (no
    /// schema to declare) and is skipped, exactly as the fs sink does.
    ///
    /// This is the single encoder both hosts share: the native [`crate::sink`]
    /// writes each file to a directory, the wasm host returns them to JS.
    ///
    /// # Errors
    /// Parquet encode or manifest YAML serialization failures.
    pub fn to_files(&self) -> Result<Vec<GraphArFile>, EncodeError> {
        let mut out = Vec::new();
        for v in &self.vertices {
            if let Some(bytes) = batches_to_parquet(&v.batches)? {
                out.push(GraphArFile {
                    rel_path: format!("vertex/{}.parquet", v.label),
                    bytes,
                });
            }
        }
        for e in &self.edges {
            let dir = format!("{}_{}_{}", e.src_type, e.label, e.dst_type);
            for (orient, batches) in [("by_source", &e.by_source), ("by_target", &e.by_target)] {
                if let Some(bytes) = batches_to_parquet(batches)? {
                    out.push(GraphArFile {
                        rel_path: format!("edge/{dir}/{orient}.parquet"),
                        bytes,
                    });
                }
            }
        }
        for manifest in self.manifests()? {
            out.push(GraphArFile {
                rel_path: manifest.rel_path,
                bytes: manifest.yaml.into_bytes(),
            });
        }
        Ok(out)
    }
}

/// Encode `batches` to a Parquet byte buffer (uncompressed — `parquet`'s default
/// props; the discovery reader decodes any valid Parquet). Returns `None` for an
/// empty batch set (a 0-row type has no schema to declare).
///
/// # Errors
/// Parquet encode failures.
pub fn batches_to_parquet(
    batches: &[RecordBatch],
) -> Result<Option<Vec<u8>>, parquet::errors::ParquetError> {
    let Some(first) = batches.first() else {
        return Ok(None);
    };
    let mut buf = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buf, first.schema(), None)?;
    for batch in batches {
        writer.write(batch)?;
    }
    writer.close()?;
    Ok(Some(buf))
}
