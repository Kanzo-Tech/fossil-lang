//! The graph a program writes, as a TYPE: `⊢ P : S₁ × … × Sₙ → Graph⟨G⟩`, and
//! [`graph_type`] is `G`.
//!
//! `G` is a property-graph type in the sense of PG-Schema: vertex types keyed by
//! their subject, properties with a datatype and a cardinality, and edge types
//! with a source and a destination. It is computed once per program, as the
//! checker computes any other type, and it is plain data: every consumer — the
//! lowering that emits the edges, the run, the editor's `mapping.ttl`, a sink —
//! reads this one value, so no two of them can disagree about the graph.
//!
//! Two places give it, in this order:
//!
//! 1. **The program's types**, declared (`@iri("…") type Person`) or imported
//!    (`type { … } := io.shex("shop.shex")`), as
//!    [`program_shapes`](crate::shapes::program_shapes) gathers them. They are
//!    what the checker resolves every mapping against, so they are the graph's.
//! 2. `io.rdf(schema = io.shex(…))` — an RDF input whose shape is also the
//!    graph's, for a program that reads a graph and writes one back. One such
//!    shape per program: a second, different one is refused, not merged.
//!
//! A program naming neither has the empty type, under which every property a
//! mapping writes is a column of its vertex.

use fossil_base::providers::{RowReader, installed, provider};
use fossil_base::{Db, SourceFile};
use fossil_graph_schema::{GraphSchema, Problem};

use crate::def_map::def_map;
use crate::shapes::{decoded_document, program_shapes};

/// `G`, the type of the graph `file` writes.
///
/// # Errors
///
/// A document the type names that does not decode — unregistered, named by no
/// provider or by one that reads no types, or unparseable; the checker reports
/// each at the span that named it — and `shape/more-than-one-output` for two
/// different `io.rdf` schemas.
#[salsa::tracked(returns(clone))]
pub fn graph_type(db: &dyn Db, file: SourceFile) -> Result<GraphSchema, Problem> {
    if let Some(shapes) = program_shapes(db, file)? {
        return Ok(shapes.to_graph_schema());
    }

    let mut schema: Option<(Option<&str>, &str)> = None;
    for s in def_map(db, file).sources(db) {
        let reads_a_graph = s
            .constructor
            .as_deref()
            .and_then(|c| provider(installed(db), c))
            .is_some_and(|p| p.reads_rows == Some(RowReader::Materialised));
        let Some(arg) = s.schema_arg.as_ref().filter(|_| reads_a_graph) else {
            continue;
        };
        match &schema {
            Some((_, existing)) if *existing != arg.as_str() => {
                return Err(Problem::MoreThanOneOutput {
                    first: (*existing).to_string(),
                    second: arg.to_string(),
                });
            }
            _ => schema = Some((s.schema_provider.as_deref(), arg.as_str())),
        }
    }

    match schema {
        Some((provider, document)) => {
            Ok(decoded_document(db, file, provider, document)?.to_graph_schema())
        }
        None => Ok(GraphSchema::default()),
    }
}
