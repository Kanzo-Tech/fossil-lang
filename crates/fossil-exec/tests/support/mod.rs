// These items are `pub(crate)` (a private module ⇒ `unreachable_pub` wants
// `pub(crate)`), which trips the inverse `redundant_pub_crate` nursery lint —
// silenced the way the rest of the workspace silences it.
#![allow(clippy::redundant_pub_crate)]

//! The host half these integration tests owe the checker: a filesystem, every
//! provider row `fossil_hir::providers` publishes, and the shape documents a
//! program names, put into the database before any query goes looking for them.

// The module is shared by eight test binaries and no binary uses all of it.
#![allow(dead_code)]

use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System, register_file};

/// A database over a host with every provider row, holding `program` at `program_path` and every
/// `(path, text)` document registered under its own path.
///
/// The registry key the compiler looks a document up under is the path the
/// program writes joined onto the program's directory
/// (`fossil_hir::def_map::resolve_relative`). Every program here sits at the
/// crate root (`"hello.fossil"`), whose parent is empty, so the key IS the name
/// the `io.shex("…")` wrote. A document registered under any other string reads
/// exactly like a document nobody registered.
///
/// Registration takes the TEXT, not a path: nothing here touches the disk, so a
/// shape document needs no fixture file.
pub(crate) fn db_with_shapes(
    program: &str,
    program_path: &str,
    documents: &[(&str, &str)],
) -> (FossilDb, SourceFile) {
    let system: Arc<dyn System> = Arc::new(NativeSystem::with_providers(
        fossil_hir::providers::PROVIDERS,
    ));
    let mut db = FossilDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), program_path.to_string());
    for (path, text) in documents {
        let doc = SourceFile::new(&db, (*text).to_string(), (*path).to_string());
        register_file(&mut db, (*path).to_string(), doc);
    }
    (db, file)
}

/// Every diagnostic the file produces, as every host reads them.
///
/// A mapping that does not compile makes `execute_*` return
/// `Plan("the mapping did not compile; see the reported diagnostics")` — a
/// message that names no diagnostic. This is how a test says which.
pub(crate) fn diagnostics(db: &FossilDb, file: SourceFile) -> Vec<String> {
    fossil_mir::program_diagnostics(db, file)
        .iter()
        .map(fossil_base::Diagnostic::message)
        .collect()
}

/// A [`fossil_storage::Host`] that vends nothing: every store a test needs is
/// registered on the [`fossil_storage::Storage`] by hand.
#[derive(Debug)]
pub(crate) struct NoHost;

impl fossil_storage::Host for NoHost {
    fn connections(
        &self,
    ) -> futures::future::BoxFuture<
        'static,
        Result<std::collections::HashMap<String, String>, fossil_storage::HostError>,
    > {
        Box::pin(async { Ok(std::collections::HashMap::new()) })
    }
    fn credentials(
        &self,
        _: &fossil_storage::Scope,
        _: fossil_storage::Access,
    ) -> futures::future::BoxFuture<
        'static,
        Result<Vec<fossil_storage::StorageCredential>, fossil_storage::HostError>,
    > {
        Box::pin(async {
            Err(fossil_graph_schema::Foreign::named("Error", "these tests vend nothing").into())
        })
    }
}

/// A [`fossil_storage::Host`] that records every credential it is asked for
/// and vends `write` on a job only: `prefixes` S3 prefixes, `s3://lake/<job>/<n>/`.
/// A read is refused, so a test routes its sources by hand.
///
/// Building a store from a vended credential sends nothing; only a request
/// through it would, and a test that writes through one is a test with a
/// network.
#[derive(Debug)]
pub(crate) struct Vending {
    pub(crate) prefixes: usize,
    pub(crate) asked: std::sync::Mutex<Vec<(fossil_storage::Scope, fossil_storage::Access)>>,
}

impl Vending {
    pub(crate) const fn new(prefixes: usize) -> Self {
        Self {
            prefixes,
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Every `(scope, access)` asked for, in order.
    pub(crate) fn asked(&self) -> Vec<(fossil_storage::Scope, fossil_storage::Access)> {
        self.asked.lock().expect("lock").clone()
    }
}

impl fossil_storage::Host for Vending {
    fn connections(
        &self,
    ) -> futures::future::BoxFuture<
        'static,
        Result<std::collections::HashMap<String, String>, fossil_storage::HostError>,
    > {
        Box::pin(async { Ok(std::collections::HashMap::new()) })
    }
    fn credentials(
        &self,
        scope: &fossil_storage::Scope,
        access: fossil_storage::Access,
    ) -> futures::future::BoxFuture<
        'static,
        Result<Vec<fossil_storage::StorageCredential>, fossil_storage::HostError>,
    > {
        self.asked
            .lock()
            .expect("lock")
            .push((scope.clone(), access));
        let fossil_storage::Scope::Job(job) = scope else {
            return Box::pin(async {
                Err(fossil_graph_schema::Foreign::named("Error", "this host vends no read").into())
            });
        };
        let vended = (0..self.prefixes)
            .map(|n| fossil_storage::StorageCredential {
                prefix: format!("s3://lake/{job}/{n}/"),
                config: [
                    ("s3.access-key-id", "K"),
                    ("s3.secret-access-key", "S"),
                    ("client.region", "us-east-1"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into()))
                .collect(),
            })
            .collect();
        Box::pin(async move { Ok(vended) })
    }
}

/// Where [`write_in_memory`] writes.
pub(crate) const MEMORY_DEST: &str = "mem://out/graph/";

/// [`fossil_sinks::graphar::write`] into an in-memory store, and every file it wrote, by
/// path under [`MEMORY_DEST`].
pub(crate) async fn write_in_memory(
    graph: &fossil_sinks::Graph,
) -> (
    fossil_sinks::graphar::Written,
    std::collections::BTreeMap<String, Vec<u8>>,
) {
    use futures::TryStreamExt;
    use object_store::{ObjectStore, ObjectStoreExt};
    let mut storage = fossil_storage::Storage::new(Arc::new(NoHost));
    let out = Arc::new(object_store::memory::InMemory::new());
    storage
        .with_store("mem://out/", Arc::clone(&out) as Arc<dyn ObjectStore>)
        .expect("route the in-memory store");
    let written = fossil_sinks::graphar::write(
        graph,
        &datafusion::prelude::SessionContext::new(),
        &storage,
        MEMORY_DEST,
    )
    .await
    .expect("write");
    let mut files = std::collections::BTreeMap::new();
    let listed: Vec<_> = out.list(None).try_collect().await.expect("list");
    for meta in listed {
        let bytes = out
            .get(&meta.location)
            .await
            .expect("get")
            .bytes()
            .await
            .expect("bytes");
        let path = meta
            .location
            .as_ref()
            .strip_prefix("graph/")
            .expect("under the destination")
            .to_string();
        files.insert(path, bytes.to_vec());
    }
    (written, files)
}

/// Read every provider source's bytes from the local filesystem and register
/// the decoded relations in `ctx` — what the executor does through its
/// `Storage`, for a test that drives `execute_graph` itself.
pub(crate) fn register_provider_sources(
    ctx: &datafusion::prelude::SessionContext,
    db: &FossilDb,
    file: SourceFile,
) {
    for binding in fossil_exec::provider_bindings(db, file) {
        let turtle = std::fs::read_to_string(&binding.uri)
            .unwrap_or_else(|e| panic!("read RDF source `{}`: {e}", binding.uri));
        fossil_exec::register_rdf(ctx, &binding, &turtle).expect("register the RDF source");
    }
}

// ───────────────────────────────────────────────────────────────────── graph

/// Where a test finds a file a run wrote, by its path under the graph's root.
pub(crate) type Read<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

/// `fossil.graph.yml` and the documents it names, parsed into `GraphAr`'s own
/// model by `graphar-info` and validated — never through the writer's types.
pub(crate) fn graph(read: Read<'_>) -> graphar_info::GraphInfo {
    let text = |name: &str| {
        String::from_utf8(read(name).unwrap_or_else(|| panic!("the run wrote `{name}`")))
            .expect("an information file is UTF-8")
    };
    let graph = graphar_info::GraphInfo::load(&text("fossil.graph.yml"), |name| Ok(text(name)))
        .expect("fossil.graph.yml is a GraphAr graph");
    graph.validate().expect("the graph is valid");
    graph
}

/// An `int64` count file, as `GraphAr` writes it.
pub(crate) fn count(read: Read<'_>, path: &str) -> u64 {
    let bytes = read(path).unwrap_or_else(|| panic!("the run wrote `{path}`"));
    u64::try_from(i64::from_le_bytes(
        bytes.as_slice().try_into().expect("a count is eight bytes"),
    ))
    .expect("a count is not negative")
}

/// The vertices of `ty`, from its `vertex_count` file.
pub(crate) fn vertex_count(read: Read<'_>, graph: &graphar_info::GraphInfo, ty: &str) -> u64 {
    let info = graph
        .vertices
        .iter()
        .find(|v| v.r#type == ty)
        .unwrap_or_else(|| panic!("the graph has no vertex type `{ty}`"));
    count(read, &graphar_parquet::paths::vertex_count(info))
}

/// The edges of `triple`, from its `edge_count` files: none are written for a
/// type with no source vertex.
pub(crate) fn edge_count(read: Read<'_>, graph: &graphar_info::GraphInfo, triple: &str) -> u64 {
    let info = graph
        .edges
        .iter()
        .find(|e| e.triple() == triple)
        .unwrap_or_else(|| panic!("the graph has no edge type `{triple}`"));
    let sources = vertex_count(read, graph, &info.src_type);
    (0..sources.div_ceil(info.src_chunk_size))
        .map(|part| {
            count(
                read,
                &graphar_parquet::paths::edge_count(info, &info.adj_lists[0], part),
            )
        })
        .sum()
}
