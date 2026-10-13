//! The program-resident OUTPUT descriptor, decoded from the document the
//! checker read — for every host that runs a program.
//!
//! The text comes out of the document registry and never off a disk or an
//! argument: a host registers what `fossil_hir::documents::missing_documents`
//! reports, and the run decodes it with the checker's own decode — one
//! function, so a run and a check cannot read two documents
//! (`/docs/design/incremental`).

use fossil_base::providers::provider;
use fossil_base::{Db, SourceFile};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_graph_schema::{Failure, Problem};
use fossil_hir::def_map::def_map;

/// Resolve the output descriptor `file` names.
///
/// Two places name one, in this order:
///
/// 1. **The program's types** — declared (`@iri("…") type Person`) or imported
///    (`type { … } := io.shex("shop.shex")`), as
///    [`fossil_hir::shapes::program_shapes`] gathers them. It is what the
///    CHECKER resolves the mappings against, so it wins: the types the run
///    classifies edges with have to be the ones the program compiled against,
///    or a CSV program gets `ACCEPT_ALL_DEFAULT` and emits zero edges.
/// 2. `io.rdf(schema = io.shex(…))` — an RDF input whose shape doubles as the
///    output contract, for a program that reads a graph and writes one back.
///
/// v1: one shape per program; a second, different `io.rdf` schema is refused,
/// not merged. A program naming neither accepts everything.
///
/// # Errors
/// The document is unregistered, named by no provider or by one that reads no
/// types, has an extension the row declines, or fails to decode.
pub fn output_descriptor(db: &dyn Db, file: SourceFile) -> Result<OutputDescriptorKind, Failure> {
    if let Some(shapes) = fossil_hir::shapes::program_shapes(db, file)? {
        return Ok(OutputDescriptorKind::Lowered(shapes.to_graph_schema()));
    }

    let def_map = def_map(db, file);

    let mut schema: Option<(Option<&str>, &str)> = None;
    for s in def_map.sources(db) {
        let reads_a_graph = s
            .constructor
            .as_deref()
            .and_then(|c| provider(fossil_base::providers::installed(db), c))
            .is_some_and(|p| p.reads_rows == Some(fossil_base::RowReader::Materialised));
        let Some(arg) = s.schema_arg.as_ref().filter(|_| reads_a_graph) else {
            continue;
        };
        match &schema {
            Some((_, existing)) if *existing != arg.as_str() => {
                return Err(Problem::MoreThanOneOutput {
                    first: (*existing).to_string(),
                    second: arg.to_string(),
                }
                .into());
            }
            _ => schema = Some((s.schema_provider.as_deref(), arg.as_str())),
        }
    }

    match schema {
        Some((provider, document)) => decode(db, file, provider, document),
        None => Ok(OutputDescriptorKind::ACCEPT_ALL_DEFAULT),
    }
}

/// Decode one registered document through the checker's own decode,
/// [`fossil_hir::shapes::decoded_document`]: the same row, the same registry
/// key and the same tracked read of the text the program compiled against.
fn decode(
    db: &dyn Db,
    file: SourceFile,
    constructor: Option<&str>,
    document: &str,
) -> Result<OutputDescriptorKind, Failure> {
    let shapes = fossil_hir::shapes::decoded_document(db, file, constructor, document)?;
    Ok(OutputDescriptorKind::Lowered(shapes.to_graph_schema()))
}
