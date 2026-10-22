//! The run a host drives: one compiled program, the documents it names, the
//! sources it reads, and the graph it writes.
//!
//! An [`Executor`] holds one compiled program and is a document workspace: it
//! reports the documents the program names and does not hold
//! ([`Executor::missing_documents`]), the host reads them and registers each
//! one, and only then are the sources listed and the run executed — against the
//! program's graph type ([`fossil_hir::graph_type::graph_type`]), decoded from
//! those registered documents, which is what the checker read. The host's profile is written in the same way the editor's
//! workspace takes it ([`Executor::register_inferred_descriptor`],
//! [`Executor::register_undescribed`]), so the run checks the program against
//! the columns the editor checked it against. [`Executor::execute`] builds a `DataFusion` plan
//! (`lower_to_mir_pg` → `execute_graph`), writes the graph into its [`Output`]
//! with the [`Sink`] the host chose, and answers what it [`Ran`].
//!
//! ## Storage seam
//! Every byte goes through a [`Storage`](fossil_storage::Storage): the sources `DataFusion` reads, the
//! RDF a provider decodes, and the files the run writes. A host with storage
//! builds it from the credentials its `Host` vends, so `DataFusion` reads a
//! remote source by range requests rather than whole; a host with none holds
//! the files in memory. Both are `object_store` stores, so the run cannot tell
//! them apart.
//!
//! ## Two axes and a limit
//! Where a run reads is the [`Storage`](fossil_storage::Storage) it is handed; where it writes is its
//! [`Output`], one-to-one with the store the graph goes into — the job's, by
//! the `write` credential vended on it, or an `InMemory`, for which no write
//! credential is asked. `limit` caps the rows read from each source scan, at
//! the scan, so an edge to a vertex outside the sample is dropped and counted
//! like any other. `/docs/design/three-hosts` argues the shape.
//!
//! ## One host
//! `fossil-df-wasm` is a `wasm-bindgen` shell over this, in the browser and in
//! Node, and it is the only host that writes a graph. This is target-agnostic:
//! native tests drive it on a current-thread runtime, the browser on its own
//! event loop.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use datafusion::error::DataFusionError;
use datafusion::execution::context::SessionContext;
use fossil_base::{FossilDb, FsError, Provider, SourceFile, System};
use fossil_graph_schema::{Failure, GraphSchema, Problem, Related};
use fossil_hir::documents::MissingDocument;
use fossil_hir::lineage::Input;
use fossil_storage::{Access, Scope, Storage};
use futures::TryStreamExt;
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};

use crate::RunReport;
use crate::memory::{BUDGET, Budget, Refusal};
use fossil_sinks::Sink;

/// Where a run writes its graph — one value per `object_store` store it can
/// go into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// An `object_store` `InMemory` under [`MEMORY`]; the run answers the
    /// files ([`Ran::Memory`]) and asks the host for no `write` credential.
    Memory,
    /// The cloud store the host vends `write` on for `job` — exactly one
    /// prefix — and the graph is written under it ([`Ran::Storage`]).
    Storage {
        /// The scope the `write` credential is vended on.
        job: String,
    },
}

/// Where an [`Output::Memory`] run writes; its [`RunReport::location`].
pub const MEMORY: &str = "memory://graph/";

/// What a run answers, one variant per [`Output`].
#[derive(Debug)]
pub enum Ran {
    /// Written under the job's prefix, which the report names.
    Storage(RunReport),
    /// Written into memory, and every file of it.
    Memory {
        report: RunReport,
        files: Vec<GraphFile>,
    },
}

impl Ran {
    /// The report, whichever store the graph went into.
    #[must_use]
    pub const fn report(&self) -> &RunReport {
        match self {
            Self::Storage(report) | Self::Memory { report, .. } => report,
        }
    }
}

/// One file of a graph written into memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphFile {
    /// Relative to the graph's root: `fossil.graph.yml`,
    /// `vertex/<Type>/vertex_count`, `vertex/<Type>/<group>/chunk0`, …
    pub path: String,
    pub bytes: Bytes,
}

/// Minimal [`System`] for the executor host. The executor reads sources through
/// the object-store / provider seams and documents through the registry, never
/// through `System::read_file`, and touches no clock on its path — so
/// `read_file` is unreachable (returns `NotFound`) and `now` returns the
/// wasm-safe `UNIX_EPOCH` placeholder (`SystemTime::now()` panics on
/// `wasm32-unknown-unknown`).
///
/// `providers` installs the type-reading rows: a mapping header names a bare
/// type imported by `type { … } := io.shex("…")`, so the vertex label
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
        fossil_hir::providers::PROVIDERS
    }
}

/// One compiled program and the documents registered for it — the
/// target-agnostic core behind `fossil-df-wasm`'s `FossilExecutor`.
///
/// The order is forced: parse → ask which
/// documents the program names → register them → list sources and run. The
/// graph type is the checker's own query over the registry, so it is always
/// the document the checker read.
pub struct Executor {
    db: FossilDb,
    file: SourceFile,
    sink: Sink,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("connections", fossil_base::connections(&self.db))
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
            sink: Sink::GraphAr,
        }
    }

    /// The format a run writes: [`Sink::GraphAr`] until the host chooses
    /// another, as it chooses the storage.
    pub const fn set_sink(&mut self, sink: Sink) {
        self.sink = sink;
    }

    /// The connections (`{ name: baseUrl }`) every `@name/…` a document or
    /// source names expands against — [`fossil_base::set_connections`]. They
    /// move locations, never registry keys.
    pub fn set_connections(&mut self, connections: BTreeMap<String, String>) {
        fossil_base::set_connections(&mut self.db, connections);
    }

    /// The documents the program names that are not registered yet.
    #[must_use]
    pub fn missing_documents(&self) -> Vec<MissingDocument> {
        fossil_hir::documents::missing_documents(&self.db, self.file)
    }

    /// Register a fetched document under the key [`Self::missing_documents`]
    /// reported for it.
    pub fn register_document(&mut self, key: &str, text: &str) {
        fossil_base::register_document(&mut self.db, key, text);
    }

    /// What the program reads — [`fossil_hir::lineage::inputs`], the same list every
    /// host introspects; its data inputs are what [`Self::execute`] reads.
    #[must_use]
    pub fn inputs(&self) -> Vec<Input> {
        fossil_hir::lineage::inputs(&self.db, self.file)
    }

    /// Record the columns the host's profile found for a source, as the JSON of
    /// an `InferredDescriptor` — what the editor's workspace takes under the
    /// same name, so the run checks against the columns the editor did.
    ///
    /// # Errors
    /// What [`fossil_base::columns_from_json`] refuses. Nothing is recorded on
    /// error.
    pub fn register_inferred_descriptor(&mut self, descriptor_json: &str) -> Result<(), Failure> {
        let descriptor = fossil_base::columns_from_json(descriptor_json)?;
        fossil_base::describe_columns(&mut self.db, descriptor);
        Ok(())
    }

    /// Record that the host's profile could not describe the source the program
    /// wrote as `key`, and the `Problem` it answered, as JSON.
    ///
    /// # Errors
    /// What [`fossil_base::refusal_from_json`] refuses. Nothing is recorded on
    /// error.
    pub fn register_undescribed(&mut self, key: &str, problem_json: &str) -> Result<(), Failure> {
        let refused = fossil_base::refusal_from_json(problem_json)?;
        fossil_base::describe(&mut self.db, key, refused);
        Ok(())
    }

    /// `G`, the type of the graph the program writes, or the problem of a
    /// document it names that does not decode.
    fn graph_type(&self) -> Result<GraphSchema, Failure> {
        Ok(fossil_hir::graph_type::graph_type(&self.db, self.file)?)
    }

    /// Run the program on `DataFusion`, reading through `storage`, and write
    /// the graph into `output`: route each source — through its connection's
    /// credential, or as a public URL — register the stores, execute the graph
    /// with at most `limit` rows read from each source scan, and write it with
    /// the [`Sink`] of [`Self::set_sink`].
    ///
    /// # Errors
    /// A [`Failure`] with its code: for [`Output::Storage`], what the host
    /// refused when asked for `write` on the job, or
    /// `storage/ambiguous-prefix` when it vends more than one prefix;
    /// `run/over-budget` when an operator asked for more than the run's
    /// [`BUDGET`](crate::memory::BUDGET) had left, before anything is written;
    /// `run/does-not-compile`, relating the program's diagnostics; a
    /// `storage/…` code for a source that will not route; `source/not-found`
    /// for one whose file is not in its store; `engine/failed`, with
    /// `DataFusion`'s error as the cause; or what the sink refused.
    pub async fn execute(
        &self,
        storage: &mut Storage,
        output: &Output,
        limit: Option<usize>,
    ) -> Result<Ran, Failure> {
        let (db, file) = (&self.db, self.file);
        // Refused before anything is routed or read.
        self.graph_type()?;
        let (dest, memory) = match output {
            Output::Storage { job } => (granted(storage, job).await?, None),
            Output::Memory => {
                let memory = Arc::new(InMemory::new());
                storage.with_store(MEMORY, Arc::clone(&memory) as Arc<dyn ObjectStore>)?;
                (MEMORY.to_string(), Some(memory))
            }
        };

        for source in fossil_hir::lineage::inputs(db, file)
            .into_iter()
            .filter(|i| i.role == fossil_hir::lineage::Role::Data)
        {
            storage
                .route(&source.location, source.connection.as_deref())
                .await?;
        }

        // No operator in any plan of this session spawns: the browser has no
        // Tokio runtime to spawn on. `session` says how, and what it cannot prove.
        // `budget` is kept to ask what it refused: an operator told no can fail
        // with an error of its own — a sort, that it has no disk to spill to.
        let budget = Arc::new(Budget::new(BUDGET));
        let ctx = crate::session::session_within(Arc::clone(&budget), limit);
        for (authority, store) in storage.stores() {
            ctx.register_object_store(authority, store);
        }
        register_rdf_sources(&ctx, db, file, storage).await?;

        let graph = crate::execute_graph(&ctx, db, file)
            .await
            .map_err(|e| engine_failure(e, budget.refusal(), db, file))?;

        let dropped = match self.sink {
            // The writer counts what it writes on the same session, under the
            // same pool: a profile that does not fit is refused as any operator
            // is, before the chunk it is the footer of.
            Sink::GraphAr => {
                fossil_sinks::graphar::write(&graph, &ctx, storage, &dest)
                    .await
                    .map_err(|e| match e {
                        fossil_sinks::graphar::WriteError::Engine(e) => {
                            engine_failure(e, budget.refusal(), db, file)
                        }
                        other => other.into(),
                    })?
                    .dropped
            }
            #[cfg(feature = "ntriples")]
            Sink::NTriples => {
                fossil_sinks::ntriples::write(&graph, storage, &dest).await?;
                graph.dropped()
            }
        };
        drop(ctx);
        let report = RunReport {
            location: dest,
            dropped,
        };
        Ok(match memory {
            None => Ran::Storage(report),
            Some(memory) => Ran::Memory {
                report,
                files: held(&memory).await?,
            },
        })
    }
}

/// The one prefix `storage`'s host vends `write` on for `job`.
async fn granted(storage: &mut Storage, job: &str) -> Result<String, Failure> {
    let scope = Scope::Job(job.to_string());
    match storage.grant(scope.clone(), Access::Write).await? {
        [prefix] => Ok(prefix.clone()),
        prefixes => Err(Failure::new(Problem::AmbiguousPrefix {
            scope: scope.to_string(),
            count: prefixes.len() as u64,
        })
        .with_help("a run writes under exactly one prefix")),
    }
}

/// Every file of the graph written into `memory`, by its path under
/// [`MEMORY`] — which is the store's root, so a key is the path.
async fn held(memory: &InMemory) -> Result<Vec<GraphFile>, Failure> {
    let unreadable = |e: object_store::Error| {
        Failure::new(Problem::Unreachable {
            location: MEMORY.to_string(),
        })
        .caused_by(e)
    };
    let listed: Vec<_> = memory.list(None).try_collect().await.map_err(unreadable)?;
    let mut files = Vec::with_capacity(listed.len());
    for meta in listed {
        let bytes = memory
            .get(&meta.location)
            .await
            .map_err(unreadable)?
            .bytes()
            .await
            .map_err(unreadable)?;
        files.push(GraphFile {
            path: meta.location.to_string(),
            bytes,
        });
    }
    Ok(files)
}

/// Decode + register every provider (RDF) source through
/// [`crate::register_rdf`], its bytes read through `storage`.
async fn register_rdf_sources(
    ctx: &SessionContext,
    db: &dyn fossil_base::Db,
    file: SourceFile,
    storage: &Storage,
) -> Result<(), Failure> {
    for binding in crate::provider_bindings(db, file) {
        let bytes = storage.get(&binding.uri).await?;
        let turtle = std::str::from_utf8(&bytes).map_err(|e| {
            Failure::new(Problem::NotUtf8 {
                location: binding.uri.clone(),
            })
            .caused_by(e)
        })?;
        crate::register_rdf(ctx, &binding, turtle).map_err(|e| {
            Failure::new(Problem::SourceUnparseable {
                location: binding.uri.clone(),
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
