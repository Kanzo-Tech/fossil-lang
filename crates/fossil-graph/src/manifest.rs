//! `GraphAr` manifest reader.
//!
//! The writer side (`fossil-sinks::manifest`) is the source of truth for the
//! `VertexInfo` / `EdgeInfo` / `PropertyGroup` shapes — this module just
//! parses the YAML file (or in-memory bytes) into those structs and exposes
//! lookup helpers used by every verb.
//!
//! Stays generic-data so the write path and the read path round-trip on the
//! same types — no parallel "`GraphArReader`" structs that drift from the
//! writer's emission ([[`feedback_no_duplicate_logic_across_crates`]] applied
//! at crate boundary).

use crate::{GraphError, Result};

/// Parsed `GraphAr` manifest. W1 stub — re-exposes the fossil-sinks manifest
/// types for verbs to consume. W2 adds a YAML loader + a `lookup_type` /
/// `lookup_edge` index.
#[derive(Debug, Clone)]
pub struct Manifest {
    // Placeholder — W2 fills this in with the deserialized fossil-sinks
    // VertexInfo + EdgeInfo collections plus precomputed name → index maps.
}

impl Manifest {
    /// Parse a `GraphAr` manifest from YAML bytes. W1 stub.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::InvalidManifest`] when the YAML cannot be parsed
    /// or required fields are missing.
    pub const fn from_yaml(_bytes: &[u8]) -> Result<Self> {
        Err(GraphError::NotImplemented("manifest::from_yaml"))
    }
}
