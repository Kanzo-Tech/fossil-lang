//! The run a host drives: one compiled program, the documents it names, the
//! sources it reads, and the corpus it writes.
//!
//! An [`Executor`] holds one compiled program and is a document workspace: it
//! reports the documents the program names and does not hold
//! ([`Executor::missing_documents`]), the host reads them and registers each
//! one, and only then are the sources listed and the run executed — against the
//! output descriptor decoded from those registered documents, which is what the
//! checker read. [`Executor::execute`] builds a `DataFusion` plan
//! (`lower_to_mir_pg` → `execute_graph`), writes the corpus under the
//! destination with [`crate::write()`], and answers the [`RunReport`].
//!
//! ## Storage seam
//! Every byte goes through a [`Storage`](fossil_storage::Storage): the sources `DataFusion` reads, the
//! RDF a provider decodes, and the files the run writes. A host with storage
//! builds it from the credentials its `Host` vends, so `DataFusion` reads a
//! remote source by range requests rather than whole; a host with none holds
//! the files in memory. Both are `object_store` stores, so the run cannot tell
//! them apart.
//!
//! ## One host
//! `fossil-df-wasm` is a `wasm-bindgen` shell over this, in the browser and in
//! Node, and it is the only host that writes a corpus. This is target-agnostic:
//! native tests drive it on a current-thread runtime, the browser on its own
//! event loop.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use datafusion::error::DataFusionError;
use datafusion::execution::context::SessionContext;
use fossil_base::{FossilDb, FsError, Provider, SourceFile, System};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_graph_schema::{Failure, Problem, Related};
use fossil_hir::documents::MissingDocument;
use fossil_sinks::manifest::MANIFEST_FILE;
use fossil_storage::{Access, Scope, Storage};
use url::Url;

use crate::memory::{BUDGET, Budget, Refusal};
use crate::{RunReport, SourceFormat};

/// Minimal [`System`] for the executor host. The executor reads sources through
/// the object-store / provider seams and documents through the registry, never
/// through `System::read_file`, and touches no clock on its path — so
/// `read_file` is unreachable (returns `NotFound`) and `now` returns the
/// wasm-safe `UNIX_EPOCH` placeholder (`SystemTime::now()` panics on
/// `wasm32-unknown-unknown`).
///
/// `providers` installs the type-reading rows: a mapping header names a bare
/// type bound positionally by `type { … } := io.shex("…")`, so the vertex label
/// and every predicate come from the registered document, decoded by its row.
#[derive(Debug, Default)]
struct ExecutorSystem;

impl System for ExecutorSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound(path.display().to_string()))
    }
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}

/// One compiled program and the documents registered for it — the
/// target-agnostic core behind `fossil-df-wasm`'s `FossilExecutor`.
///
/// The order is forced: parse → ask which
/// documents the program names → register them → list sources and run. The
/// output descriptor is decoded from the registry on each call, so it is always
/// the document the checker read.
pub struct Executor {
    db: FossilDb,
    file: SourceFile,
    connections: HashMap<String, String>,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("connections", &self.connections)
            .finish_non_exhaustive()
    }
}

impl Executor {
    /// Compile `program`, with no connections until [`Self::set_connections`].
    ///
    /// The program has no location, so a relative source or document it names
    /// resolves against nothing; [`Self::at`] gives it one.
    #[must_use]
    pub fn new(program: &str) -> Self {
        Self::at(program, "program.fossil")
    }

    /// Compile `program` as the file at `path` — a URL, usually — so a
    /// relative `io.csv("data/users.csv")` or `io.shex("shape.shex")` resolves
    /// beside it, as it would for any reader of that file.
    #[must_use]
    pub fn at(program: &str, path: &str) -> Self {
        let system: Arc<dyn System> = Arc::new(ExecutorSystem);
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, program.to_string(), path.to_string());
        Self {
            db,
            file,
            connections: HashMap::new(),
        }
    }

    /// The map (`{ name: baseUrl }`) every `@name/…` a document or source names
    /// expands against. It moves locators, never registry keys.
    // The hasher is not ours to choose: the map arrives from
    // `parse_connections(&JsValue)`, which builds a plain `HashMap`.
    #[allow(clippy::implicit_hasher)]
    pub fn set_connections(&mut self, connections: HashMap<String, String>) {
        self.connections = connections;
    }

    /// The documents the program names that are not registered yet.
    #[must_use]
    pub fn missing_documents(&self) -> Vec<MissingDocument> {
        fossil_hir::documents::missing_documents(&self.db, self.file, &self.connections)
    }

    /// Register a fetched document under the key [`Self::missing_documents`]
    /// reported for it.
    pub fn register_document(&mut self, key: &str, text: &str) {
        fossil_base::register_document(&mut self.db, key, text);
    }

    /// The program's sources as `(locator, row-name, connection)`: what
    /// [`Self::execute`] reads. The second element is the catalogue row's name,
    /// which is what the program wrote after `io.`.
    ///
    /// # Errors
    /// The output shape document is unregistered or does not decode.
    pub fn sources(&self) -> Result<Vec<(String, String, Option<String>)>, Failure> {
        let descriptor = self.descriptor()?;
        Ok(
            crate::program_sources(&self.db, self.file, &descriptor, &self.connections)
                .into_iter()
                .map(|s| (s.uri, format_kind(&s.format).to_owned(), s.connection))
                .collect(),
        )
    }

    fn descriptor(&self) -> Result<OutputDescriptorKind, Failure> {
        crate::output_descriptor(&self.db, self.file)
    }

    /// Run the program on `DataFusion` over `storage` and write the corpus
    /// under `dest`, a prefix `storage` covers: route each source — through its
    /// connection's credential, or as a public URL — register the stores,
    /// execute the graph, and [`crate::write()`] it.
    ///
    /// # Errors
    /// A [`Failure`] with its code: `run/over-budget` when an operator asked
    /// for more than the run's [`BUDGET`](crate::memory::BUDGET) had left,
    /// before anything is written; `run/does-not-compile`, relating the
    /// program's diagnostics; `run/destination-uncovered`; a `storage/…` code
    /// for a source that will not route; `source/not-found` for one whose file
    /// is not in its store; `engine/failed`, with `DataFusion`'s error as the
    /// cause; or what [`crate::write()`] refused.
    pub async fn execute(&self, storage: &mut Storage, dest: &str) -> Result<RunReport, Failure> {
        let (db, file, connections) = (&self.db, self.file, &self.connections);
        let descriptor = self.descriptor()?;
        if !dest.ends_with('/') || !storage.covers(&format!("{dest}{MANIFEST_FILE}")) {
            return Err(Problem::DestinationUncovered {
                destination: dest.to_string(),
            }
            .into());
        }

        for source in crate::program_sources(db, file, &descriptor, connections) {
            if storage.covers(&source.uri) {
                continue;
            }
            match source.connection {
                Some(connection) => storage
                    .grant(Scope::Connection(connection), Access::Read)
                    .await
                    .map(drop),
                None => storage.public(&source.uri),
            }?;
        }

        // No operator in any plan of this session spawns: the browser has no
        // Tokio runtime to spawn on. `session` says how, and what it cannot prove.
        // `budget` is kept to ask what it refused: an operator told no can fail
        // with an error of its own — a sort, that it has no disk to spill to.
        let budget = Arc::new(Budget::new(BUDGET));
        let ctx = crate::session::session_within(Arc::clone(&budget));
        for (authority, store) in storage.stores() {
            let url = Url::parse(authority).map_err(|e| {
                Failure::new(Problem::Bug {
                    what: format!("the routed authority `{authority}` is not a URL"),
                })
                .caused_by(e)
            })?;
            ctx.register_object_store(&url, store);
        }
        register_rdf_sources(&ctx, db, file, &descriptor, storage, connections).await?;

        let graph = crate::execute_graph(&ctx, db, file, &descriptor, connections)
            .await
            .map_err(|e| engine_failure(e, budget.refusal(), db, file))?;
        drop(ctx);

        let written = crate::write(&graph, storage, dest).await?;
        Ok(RunReport {
            dest: dest.to_string(),
            dropped: written.dropped,
        })
    }
}

/// A source format as the wire names it — **the catalogue row's name**, which
/// is what a program wrote after `io.`.
///
/// The `Provider` arm carries the name rather than the literal `"rdf"` because
/// more than one row can be materialised: labelling an `io.avro` source `rdf`
/// would send it back as the RDF row. That `rdf` is the only materialised row
/// today is what would make the literal correct by coincidence.
fn format_kind(f: &SourceFormat) -> &str {
    match f {
        // `{ .. }` because `Csv` carries the `delimiter =` the program wrote.
        // It is not in the wire string on purpose: the delimiter is read by
        // `crate::read_source` off the MIR the executor already holds.
        SourceFormat::Csv { .. } => "csv",
        SourceFormat::Json => "json",
        SourceFormat::Parquet => "parquet",
        SourceFormat::Provider { name } => name.as_str(),
    }
}

/// Decode + register every provider (RDF) source through
/// [`crate::register_rdf`], its bytes read through `storage`.
///
/// [`crate::register_provider_sources`] walks the same `provider_bindings` and
/// calls the same `register_rdf` over the local filesystem, for tests; where
/// the bytes come from is the whole difference.
async fn register_rdf_sources(
    ctx: &SessionContext,
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    storage: &Storage,
    connections: &HashMap<String, String>,
) -> Result<(), Failure> {
    for binding in crate::provider_bindings(db, file, descriptor, connections) {
        let bytes = storage.get(&binding.uri).await?;
        let turtle = std::str::from_utf8(&bytes).map_err(|e| {
            Failure::new(Problem::NotUtf8 {
                locator: binding.uri.clone(),
            })
            .caused_by(e)
        })?;
        crate::register_rdf(ctx, &binding, turtle).map_err(|e| {
            Failure::new(Problem::SourceUnparseable {
                locator: binding.uri.clone(),
            })
            .caused_by(e)
        })?;
    }
    Ok(())
}

/// What an engine error means for the run, in order: the budget refused an
/// operator (whatever error the operator then raised), the program does not
/// compile (`refuse_if_poisoned`'s [`Failure`], wherever `DataFusion`
/// wrapped it), or the engine failed, its error kept as the cause.
pub(crate) fn engine_failure(
    e: DataFusionError,
    refusal: Option<Refusal>,
    db: &dyn fossil_base::Db,
    file: SourceFile,
) -> Failure {
    if let Some(refusal) = refusal {
        return Problem::from(refusal).into();
    }
    let mut next: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(error) = next {
        if let Some(found) = error.downcast_ref::<Failure>() {
            let failure = Failure::new(found.problem.clone());
            return if matches!(found.problem, Problem::DoesNotCompile {}) {
                failure.with_related(program_diagnostics(db, file))
            } else {
                failure
            };
        }
        next = error.source();
    }
    Failure::new(Problem::EngineFailed {}).caused_by(e)
}

/// The program's diagnostics as a failure relates them — file-absolute spans,
/// as `fossil_mir::program_diagnostics` answers them.
fn program_diagnostics(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<Related> {
    fossil_mir::program_diagnostics(db, file)
        .into_iter()
        .map(|d| Related {
            severity: d.severity,
            problem: d.problem,
            help: d.help,
            span: Some(d.span),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::execution::memory_pool::{MemoryConsumer, MemoryPool};

    use super::*;

    /// An operator the budget refused makes the run `run/over-budget`, with
    /// the refusal's four figures as data — whatever error the operator then
    /// raised, and the engine's own sentence is the variant's.
    #[test]
    fn a_refused_operator_fails_the_run_as_over_budget() {
        let budget = Arc::new(Budget::new(100));
        let pool: Arc<dyn MemoryPool> = Arc::clone(&budget) as _;
        let held = MemoryConsumer::new("held").register(&pool);
        held.try_grow(60).expect("inside the budget");
        let engine = MemoryConsumer::new("ExternalSorter")
            .register(&pool)
            .try_grow(50)
            .expect_err("past the budget");

        let exec = Executor::new("");
        let failure = engine_failure(engine, budget.refusal(), &exec.db, exec.file);
        assert_eq!(failure.problem.code(), "run/over-budget");
        assert_eq!(
            failure.problem,
            Problem::OverBudget {
                consumer: "ExternalSorter".to_string(),
                requested: 50,
                reserved: 60,
                budget: 100,
            }
        );
    }

    /// With no refusal, an engine error is `engine/failed` and keeps
    /// `DataFusion`'s error whole as the cause.
    #[test]
    fn any_other_engine_error_is_the_cause_of_engine_failed() {
        let exec = Executor::new("");
        let failure = engine_failure(
            DataFusionError::Plan("no".to_string()),
            None,
            &exec.db,
            exec.file,
        );
        assert_eq!(failure.problem.code(), "engine/failed");
        let original = std::error::Error::source(&failure)
            .and_then(std::error::Error::source)
            .expect("the engine's error");
        assert!(original.downcast_ref::<DataFusionError>().is_some());
    }
}
