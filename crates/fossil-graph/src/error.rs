//! Typed errors for verb dispatch and execution.
//!
//! Surface-level only — transport bindings translate `GraphError` into their
//! native error shape (JSON-RPC error for `fossil-mcp`, HTTP 4xx/5xx for
//! `fossil-http`, exit code + miette for `fossil-cli`).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphError {
    /// The manifest YAML failed to parse, or required fields are missing.
    #[error("invalid GraphAr manifest: {0}")]
    InvalidManifest(String),

    /// A verb's params struct failed to deserialise from JSON.
    #[error("invalid params for verb `{verb}`: {detail}")]
    InvalidParams { verb: &'static str, detail: String },

    /// A vertex/edge type referenced by params is not in the manifest.
    #[error("unknown {kind} `{name}` — not in manifest")]
    UnknownEntity { kind: &'static str, name: String },

    /// The underlying `DuckDB` execution failed. Carries the executor's
    /// stringified error so this crate stays free of any concrete DB dep.
    #[error("execution failed: {0}")]
    Execution(String),
}

pub type Result<T> = std::result::Result<T, GraphError>;
