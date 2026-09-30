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
//! Every byte goes through a [`Storage`]: the sources `DataFusion` reads, the
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
//! event loop. The native `fossil run` host that sat beside it was deleted on
//! 2026-09-30 — no consumer used it, and it wrote a different corpus from the
//! same program.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use datafusion::execution::context::SessionContext;
use fossil_base::{FossilDb, FsError, Provider, SourceFile, System};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_hir::documents::MissingDocument;
use fossil_sinks::manifest::MANIFEST_FILE;
use fossil_storage::{Access, Scope, Storage};
use url::Url;

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
/// target-agnostic core behind [`FossilExecutor`].
///
/// The order is the native host's and it is forced: parse → ask which
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
    #[must_use]
    pub fn new(program: &str) -> Self {
        let system: Arc<dyn System> = Arc::new(ExecutorSystem);
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, program.to_string(), "program.fossil".to_string());
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
    pub fn sources(&self) -> Result<Vec<(String, String, Option<String>)>, String> {
        let descriptor = self.descriptor()?;
        Ok(
            crate::program_sources(&self.db, self.file, &descriptor, &self.connections)
                .into_iter()
                .map(|s| (s.uri, format_kind(&s.format).to_owned(), s.connection))
                .collect(),
        )
    }

    fn descriptor(&self) -> Result<OutputDescriptorKind, String> {
        crate::output_descriptor(&self.db, self.file)
    }

    /// Run the program on `DataFusion` over `storage` and write the corpus
    /// under `dest`, a prefix `storage` covers: route each source — through its
    /// connection's credential, or as a public URL — register the stores,
    /// execute the graph, and [`crate::write()`] it.
    ///
    /// # Errors
    /// A source or the destination no store covers, `DataFusion` execution,
    /// the layout, Parquet encode, or a write — each as a message for the host.
    pub async fn execute(&self, storage: &mut Storage, dest: &str) -> Result<RunReport, String> {
        let (db, file, connections) = (&self.db, self.file, &self.connections);
        let descriptor = self.descriptor()?;
        if !dest.ends_with('/') || !storage.covers(&format!("{dest}{MANIFEST_FILE}")) {
            return Err(format!("no store covers the destination {dest}"));
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
            }
            .map_err(|e| format!("source {}: {e}", source.uri))?;
        }

        // No operator in any plan of this session spawns: the browser has no
        // Tokio runtime to spawn on. `session` says how, and what it cannot prove.
        let ctx = crate::session::session();
        for (authority, store) in storage.stores() {
            let url = Url::parse(authority).map_err(|e| format!("{authority}: {e}"))?;
            ctx.register_object_store(&url, store);
        }
        register_rdf_sources(&ctx, db, file, &descriptor, storage, connections).await?;

        let graph = crate::execute_graph(&ctx, db, file, &descriptor, connections)
            .await
            .map_err(|e| format!("execute_graph: {e}"))?;
        drop(ctx);

        let written = crate::write(&graph, storage, dest)
            .await
            .map_err(|e| e.to_string())?;
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
) -> Result<(), String> {
    for binding in crate::provider_bindings(db, file, descriptor, connections) {
        let bytes = storage
            .get(&binding.uri)
            .await
            .map_err(|e| format!("RDF source: {e}"))?;
        let turtle = std::str::from_utf8(&bytes)
            .map_err(|e| format!("RDF source `{}` is not UTF-8: {e}", binding.uri))?;
        crate::register_rdf(ctx, &binding, turtle)
            .map_err(|e| format!("register RDF `{}`: {e}", binding.uri))?;
    }
    Ok(())
}
