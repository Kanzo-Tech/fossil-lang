//! The `GraphAr` output as in-memory bytes — the dataset's manifests.
//!
//! # What this module emits, and what it stopped emitting
//!
//! [`GraphArData::manifest_files`] turns the materialised graph into the
//! dataset's **manifest YAMLs** as `(rel_path, bytes)` pairs. It used to emit
//! the payload too — one Parquet per vertex type and the CSR/CSC pair per edge —
//! and the layout pass then read every one of those back, rewrote it as tiles
//! and deleted it. Measured on com-DBLP: 115.9 MB written against 72.6 MB of
//! final corpus, **43.3 MB staged and deleted, a 1.60× write amplification**.
//!
//! So the payload is written **once**, by `fossil_layout`, out of the same
//! `RecordBatch`es this module would have encoded — through
//! `fossil_tile_writer::TileWriter`, which was a struct in this file and is a
//! crate of its own now: nothing in `fossil-df` ever called it, and the one
//! `use` in the layout pass was what put `salsa` and `DataFusion` into that
//! pass's dependency closure.
//!
//! Pure in-memory, so nothing here touches the filesystem and all of it
//! compiles to `wasm32`.

use crate::GraphArData;

/// One output file of the `GraphAr` dataset: its dataset-relative path
/// (`graph.graph.yml`, `vertex/<Type>.vertex.yml`, `vertex/<Type>/tiles.parquet`,
/// …) and its encoded bytes. The path keys both the on-disk layout (native sink)
/// and the signed-`PUT` object key (browser host).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphArFile {
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// Encoding the `GraphAr` dataset's manifests to bytes failed.
/// Filesystem errors live in [`crate::sink::SinkError`] (native-only); this
/// stays wasm-clean.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("manifest yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("tile manifest json: {0}")]
    Json(#[from] serde_json::Error),
}

impl GraphArData {
    /// The dataset's manifest YAMLs as in-memory `(rel_path, bytes)` files.
    ///
    /// **The payload is not here** — see the module header. What describes the
    /// corpus is written by this crate; what the corpus *is* comes out of
    /// `fossil_layout`'s pass, which holds the same batches and the addresses to
    /// cut them on.
    ///
    /// # Errors
    /// Manifest YAML serialization failures.
    pub fn manifest_files(&self) -> Result<Vec<GraphArFile>, EncodeError> {
        let mut out = Vec::new();
        self.try_for_each_manifest::<EncodeError>(|file| {
            out.push(file);
            Ok(())
        })?;
        Ok(out)
    }

    /// The same manifests, one at a time: each is serialized, handed to `emit`,
    /// and dropped before the next one is built.
    ///
    /// [`Self::manifest_files`] is this with a `Vec` on the end. The native
    /// [`crate::sink`] writes each one to a directory and forgets it; the wasm
    /// host collects them, because it hands JS a list.
    ///
    /// # Errors
    /// Whatever `emit` returns, or a serialization failure converted through `E`.
    pub fn try_for_each_manifest<E: From<EncodeError>>(
        &self,
        mut emit: impl FnMut(GraphArFile) -> Result<(), E>,
    ) -> Result<(), E> {
        for manifest in self.manifests().map_err(E::from)? {
            emit(GraphArFile {
                rel_path: manifest.rel_path,
                bytes: manifest.text.into_bytes(),
            })?;
        }
        Ok(())
    }
}
