//! What a run tells its caller: the manifest, plus the two facts the corpus
//! cannot hold about itself.
//!
//! `fossil run --dest <url> --output-json` prints one [`RunReport`] on stdout.
//! **It is the manifest** — the same [`GraphInfo`] / [`VertexInfo`] /
//! [`EdgeInfo`] values that were written to disk as YAML, from the same
//! [`GraphArData::manifest`] call, serialised as JSON so a host does not have to
//! fetch and parse the YAML index and every type's document to learn what it
//! just produced. A reader that opens the corpus later gets the identical
//! description off the bytes.
//!
//! # Why this is not a second description
//!
//! It was. `RunStatus` was a parallel JSON account of a dataset the manifest
//! already described, and seven of its nine fields were a second spelling:
//! `vertex_type`, `rdf_type`, `columns`, the edge types and the two adjacency
//! paths all restated `graph.yaml`, and `file` was `prefix` plus the tiling.
//! The two that were not — the row counts — are
//! [`VertexInfo::vertex_count`](fossil_sinks::manifest::VertexInfo::vertex_count)
//! and [`EdgeInfo::edge_count`](fossil_sinks::manifest::EdgeInfo::edge_count)
//! now, required and guarded.
//!
//! The duplication cost a shipped defect before it was removed: the status named
//! `vertex/<Type>.parquet`, which is deleted once the layout post-pass has run
//! (`fossil-cli`'s host), so a host was handed a path that had just stopped
//! existing. Two descriptions of one dataset can disagree with the bytes; one
//! cannot.
//!
//! # And the two facts that are genuinely not in the corpus
//!
//! [`RunReport::dest`] — the caller supplied it, and a corpus does not know where
//! it is.
//!
//! [`RunReport::dropped`] — how many rows of each edge's input did not become an
//! edge, because an endpoint named a subject no vertex carries. That is
//! provenance: a fact about the write, not about the bytes, and it is here
//! rather than in the manifest for the reasons in [`EdgeDrops`].

use fossil_sinks::manifest::{EdgeInfo, GraphInfo, VertexInfo};
use serde::{Deserialize, Serialize};

use crate::GraphArData;

/// The manifest a run produced, the destination it was written to, and what the
/// write discarded.
///
/// The three manifest fields are exactly the documents on disk:
/// [`Self::graph`] is `graph.graph.yml`, and its `vertices` / `edges` path lists
/// are positionally the [`Self::vertices`] / [`Self::edges`] entries — the index
/// names where each one went, so nothing here has to agree with a convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    /// Destination URL the corpus was written under (echoes `--dest`). The one
    /// thing the corpus cannot know about itself, because the caller chose it.
    pub dest: String,
    /// `graph.graph.yml` — the index, and the authority for every other path.
    pub graph: GraphInfo,
    /// One entry per vertex type, in `graph.vertices` order.
    pub vertices: Vec<VertexInfo>,
    /// One entry per edge type, in `graph.edges` order.
    pub edges: Vec<EdgeInfo>,
    /// One entry per edge type, in the same order — see [`EdgeDrops`].
    pub dropped: Vec<EdgeDrops>,
}

/// How many rows of one edge type's input did not become an edge.
///
/// `execute_edge` resolves both endpoints against the vertex tables with an
/// inner join, so a row naming a subject no vertex carries is discarded. **That
/// is intended and it stays** — a corpus cannot hold an edge to a vertex that is
/// not there — and until now it was also silent, at every log level.
///
/// # Why the count is here and not in the edge manifest
///
/// The ruling was to record it beside
/// [`EdgeInfo::edge_count`](fossil_sinks::manifest::EdgeInfo::edge_count), and
/// three things about the artefact argue against that, all of them checkable:
///
/// 1. **No reader can verify it.** `vertex_count` and `edge_count` earn their
///    place in the manifest by being answerable to the payload: summing the
///    tiles either reaches the declared number or does not, which is what
///    `apps/corpus`'s `declared-count` guard does. A dropped count is about rows
///    that are *not* there and never were. No guard can be written for it, and a
///    manifest field no guard can reach is a claim a reader has to take on
///    trust.
/// 2. **A third-party writer has no such number.** The format is documented so
///    that somebody else's writer can produce a corpus; `apps/corpus/guards/fixture.mjs`
///    is one, in JavaScript, and it emits edge manifests without ever performing
///    a join. A required field (and `vertex_count`'s own docblock argues at
///    length that an optional count is not a count) would make a conforming
///    corpus impossible to write without a fact only fossil's pipeline has.
/// 3. **It would not deserialise the corpora that exist.** `EdgeInfo` is parsed
///    out of any corpus fossil is pointed at, including
///    `apps/corpus/conformance/corpus`, whose edge manifest is committed and has
///    no such key.
///
/// So it sits beside [`RunReport::dest`], which is the other fact about the run
/// rather than about the bytes. Moving it into `EdgeInfo` later is a field, a
/// line in the builder and a regenerated fixture; moving it out of a published
/// format is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDrops {
    /// The edge type's manifest [`prefix`](fossil_sinks::manifest::EdgeInfo::prefix),
    /// e.g. `edge/Order_placedBy_Person/` — a pointer into [`RunReport::edges`]
    /// rather than a second spelling of the `(src, label, dst)` triple.
    pub prefix: String,
    /// Rows of this edge's input that resolved no endpoint pair. Always present,
    /// including as `0`: a reader that cannot tell «nothing was dropped» from
    /// «this writer does not count» has the silence back in another shape.
    pub dropped: u64,
}

impl RunReport {
    /// The report for a materialised graph written to `dest`.
    ///
    /// The manifest half is [`GraphArData::manifest`], which is the same call
    /// the YAML files are serialised from — so stdout and the disk cannot
    /// disagree without the builder disagreeing with itself.
    #[must_use]
    pub fn of(dest: &str, graph: &GraphArData) -> Self {
        let (index, vertices, edges) = graph.manifest();
        // Keyed through the schema, which is what `manifest()` orders the edge
        // list by, so `dropped[i]` names `edges[i]` even for a declared edge
        // type nothing materialised (which drops nothing, having read nothing).
        let dropped = edges
            .iter()
            .zip(&graph.schema.edges)
            .map(|(info, edge)| EdgeDrops {
                prefix: info.prefix.clone(),
                dropped: graph.edge_table(edge).map_or(0, |t| t.dropped),
            })
            .collect();
        Self {
            dest: dest.to_string(),
            graph: index,
            vertices,
            edges,
            dropped,
        }
    }
}
