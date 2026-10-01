//! The program-resident OUTPUT descriptor, decoded from the document the
//! checker read — for every host that runs a program.
//!
//! The text comes out of the document registry and never off a disk or an
//! argument: a host registers what `fossil_hir::documents::missing_documents`
//! reports, and the run decodes the same bytes, with the same registry row, as
//! the check. The deleted native CLI read the file again through its anchor
//! and `fossil-df-wasm` took a second copy as a `shex` string and sniffed its
//! language; either could decode a document other than the one that compiled.

use fossil_base::providers::{Capability, provider};
use fossil_base::{Db, SourceFile, file_at};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_graph_schema::{Failure, Foreign, Problem};
use fossil_hir::def_map::def_map;
use fossil_hir::documents::registry_key;

/// Resolve the output descriptor `file` names.
///
/// Two places name one, in this order:
///
/// 1. **`type { … } := io.shex("shop.shex")`** — what the CHECKER resolves the
///    mappings against, so it wins: the document the run classifies edges with
///    has to be the document the program compiled against, or a CSV program
///    gets `ACCEPT_ALL_DEFAULT` and emits zero edges.
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
    let def_map = def_map(db, file);
    if let Some((constructor, document)) = def_map.output_shape_binding(db) {
        return decode(db, file, constructor.as_deref(), &document);
    }

    let mut schema: Option<(Option<&str>, &str)> = None;
    for s in def_map.sources(db) {
        let reads_a_graph = s
            .constructor
            .as_deref()
            .and_then(|c| provider(fossil_descriptors_output::PROVIDERS, c))
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

/// Decode one registered document through the row the program named. The row
/// is selected by the constructor the program wrote and its `reads_types` is
/// the function the checker calls.
fn decode(
    db: &dyn Db,
    file: SourceFile,
    constructor: Option<&str>,
    document: &str,
) -> Result<OutputDescriptorKind, Failure> {
    let table = fossil_descriptors_output::PROVIDERS;
    let ctor = constructor.ok_or_else(|| {
        Failure::new(Problem::BareDocumentPath {
            document: document.to_string(),
        })
        .with_help("write `io.shex(\"…\")` or `io.shacl(\"…\")`")
    })?;
    let row = provider(table, ctor).ok_or_else(|| {
        Failure::new(Problem::UnknownProvider {
            constructor: ctor.to_string(),
        })
        .with_help(fossil_hir::refusals::unknown_constructor(table))
    })?;
    if !row.provides(Capability::ReadTypes) {
        return Err(Failure::new(Problem::WrongCapability {
            constructor: row.constructor(),
            capability: "read types".to_string(),
        })
        .with_help(fossil_hir::refusals::decline_capability(
            row,
            Capability::ReadTypes,
            table,
        )));
    }
    if !row.accepts(document) {
        return Err(Failure::new(Problem::WrongExtension {
            constructor: row.constructor(),
            document: document.to_string(),
        })
        .with_help(fossil_hir::refusals::decline_extension(row, document)));
    }
    let read = row.reads_types.ok_or_else(|| Problem::Bug {
        what: format!(
            "`{}` declares it reads types and has no decoder",
            row.constructor()
        ),
    })?;

    let key = registry_key(db, file, document);
    let registered = file_at(db, &key).ok_or_else(|| Problem::NotRegistered {
        document: document.to_string(),
    })?;
    let shapes = read(&key, registered.text(db)).map_err(|e| {
        Failure::new(Problem::Unparseable {
            document: document.to_string(),
            reason: None,
        })
        .caused_by(Foreign::named("Rejection", format!("{e:?}")))
    })?;
    Ok(OutputDescriptorKind::Lowered(
        shapes.to_graph_schema(&def_map(db, file).renames(db)),
    ))
}
