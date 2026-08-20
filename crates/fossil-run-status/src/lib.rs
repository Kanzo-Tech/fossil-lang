//! The `fossil run --output-json` wire contract.
//!
//! `fossil run --dest <url> --output-json` emits one [`RunStatus`] object on
//! stdout describing the `GraphAr` dataset it just wrote: per vertex type its
//! Parquet file + row count + property columns; per edge type its CSR/CSC file
//! pair + endpoints + count. A host (keasy) deserializes it to persist the job's
//! output structure WITHOUT re-introspecting the dataset.
//!
//! This crate is the SINGLE source of truth for that shape: the CLI serializes
//! [`RunStatus`], host consumers deserialize the same struct (depend on this
//! crate — it is dependency-light + WASM-clean, NOT the execution library), and
//! the `JsonSchema` derives publish the contract for TypeScript codegen (the
//! same pipeline `fossil-graph` uses for its verb result types).
//!
//! ## Stats boundary
//!
//! [`RunStatus`] carries STRUCTURE only — no column-value statistics
//! (`n_unique`/`min`/`max`/`samples`). The host's browser data plane computes
//! those on demand via DuckDB-WASM over the mounted Parquet. (Host-boundary: the
//! server ships structure, the browser owns stats.)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The status object `fossil run --output-json` writes to stdout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunStatus {
    /// Destination URL the `GraphAr` dataset was written under (echoes `--dest`).
    pub dest: String,
    /// One entry per emitted vertex type.
    pub vertices: Vec<VertexStatus>,
    /// One entry per emitted edge type (empty until a shape/Phase-B adds edges).
    pub edges: Vec<EdgeStatus>,
}

/// One vertex type — its dataset-relative Parquet, row count, and property
/// columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VertexStatus {
    /// The vertex type / `GraphAr` `type` (e.g. `Person`).
    #[serde(rename = "type")]
    pub vertex_type: String,
    /// The full RDF type IRI the mapping declares for this vertex (the shape IRI,
    /// e.g. `https://example.org/Person`); `None` when the output has no RDF
    /// type. This is the output *spec* fossil owns — the governance layer (DCAT)
    /// reads it from the manifest instead of re-deriving it from the program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdf_type: Option<String>,
    /// **Where this vertex type's rows are**, dataset-relative — and a host
    /// fetches it, so it names something that exists.
    ///
    /// Two spellings, and which one you get says whether the dataset has been
    /// tiled: a single Parquet (`vertex/Person.parquet`) from a producer that
    /// wrote one, and the chunk PREFIX (`vertex/Person/`, holding
    /// `chunk{k}.parquet` — the same string the `GraphAr` `VertexInfo`
    /// declares) once a layout pass has replaced it with chunks.
    ///
    /// It used to be the single Parquet unconditionally, and native
    /// `fossil run` deletes that file: the layout pass reads it, writes the
    /// chunks, and removes it as the last thing it does, so the status went out
    /// naming a path that had just stopped existing. The browser executor runs
    /// no layout pass, which is why the same field was truthful there and
    /// nobody noticed.
    pub file: String,
    /// Row count (Parquet footer metadata); `None` if the count query failed.
    pub count: Option<i64>,
    /// The vertex's property columns.
    pub columns: Vec<ColumnStatus>,
}

/// A property column of a [`VertexStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColumnStatus {
    /// Column / predicate local name.
    pub name: String,
    /// `GraphAr` data-type spelling (`string`, `int64`, `double`, …).
    pub data_type: String,
    /// The full RDF predicate IRI this column maps (e.g.
    /// `https://example.org/name`); `None` when the output has no RDF predicate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdf_uri: Option<String>,
    /// The XSD datatype IRI of the literal value (e.g.
    /// `http://www.w3.org/2001/XMLSchema#string`); `None` when not an RDF
    /// literal. Part of the output spec the governance layer (DCAT) consumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xsd_datatype: Option<String>,
}

/// One edge type — its CSR/CSC Parquet pair and endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EdgeStatus {
    /// The edge type / predicate local name.
    pub edge_type: String,
    /// Source vertex type.
    pub src_type: String,
    /// Destination vertex type.
    pub dst_type: String,
    /// CSR-ordered (`by_source`) Parquet, dataset-relative.
    pub by_source: String,
    /// CSC-ordered (`by_target`) Parquet, dataset-relative.
    pub by_target: String,
    /// Edge count (Parquet footer metadata); `None` if the count query failed.
    pub count: Option<i64>,
}

// ── `fossil providers` contract ─────────────────────────────────────────
//
// The host (keasy) lists the data-source constructors fossil supports so its UI
// can offer them + filter files by extension. fossil owns this set (host
// boundary): the `providers` subcommand enumerates the registry and emits this
// contract; keasy reads it over the subprocess and serves `/v1/providers`.

/// What a provider can appear as in a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Only defines a type (e.g. a schema descriptor).
    Schema,
    /// Loads data (the `io.*` source constructors).
    Data,
    /// Usable in both positions.
    Both,
}

/// One data-source provider fossil exposes: its short name, the file extensions
/// it reads, and how it can be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderInfo {
    /// Short provider name (e.g. `csv`, `json`, `parquet`).
    pub name: String,
    /// File extensions this provider reads (no leading dot).
    pub extensions: Vec<String>,
    /// Whether the provider defines a type, loads data, or both.
    pub kind: ProviderKind,
}

// ----- `refs` (host boundary) -----------------------------------------------
// The `refs` subcommand parses a program and emits its external references —
// the TYPED lineage of what the program reads. keasy consumes this (instead of
// regex-matching `@name/` in script text) to know a job's connections.

/// The position a reference plays in an `io.*` source constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefRole {
    /// The positional data URI (`io.rdf("…")`).
    Data,
    /// The `schema = io.shex("…")` argument (a shape document).
    Schema,
}

/// One external reference a program makes. `connection` is the `@conn` alias the
/// reference targets (`Some("cpi")` for `@cpi/graph.ttl`), or `None` for a direct
/// URL / local path. `path` is the remainder after the alias (or the whole
/// locator when there is no alias). This is the program's TYPED lineage — keasy
/// derives a job's connection set from the distinct `connection`s, never from a
/// regex over the script text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRefInfo {
    /// The `@conn` alias this reference targets, or `None` for a direct URL/path.
    pub connection: Option<String>,
    /// The path within the connection, or the whole locator when unaliased.
    pub path: String,
    /// Where this reference appears in the source constructor.
    pub role: RefRole,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_wire_contract() {
        let json = r#"{
            "dest": "s3://bucket/job",
            "vertices": [
                { "type": "Person", "file": "vertex/Person.parquet", "count": 5,
                  "columns": [ { "name": "name", "data_type": "string" } ] }
            ],
            "edges": [
                { "edge_type": "knows", "src_type": "Person", "dst_type": "Person",
                  "by_source": "edge/k/by_source.parquet",
                  "by_target": "edge/k/by_target.parquet", "count": 2 }
            ]
        }"#;
        let parsed: RunStatus = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.vertices[0].vertex_type, "Person");
        assert_eq!(parsed.vertices[0].count, Some(5));
        assert_eq!(parsed.vertices[0].columns[0].name, "name");
        assert_eq!(parsed.edges[0].edge_type, "knows");
        // `type` rename round-trips on serialize.
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"type\":\"Person\""), "rename lost: {back}");
    }
}
