// These items are `pub(crate)` (a private module ⇒ `unreachable_pub` wants
// `pub(crate)`), which trips the inverse `redundant_pub_crate` nursery lint —
// silenced the way the rest of the workspace silences it.
#![allow(clippy::redundant_pub_crate)]

//! The host half these integration tests owe the checker: a filesystem, every
//! provider row `fossil-descriptors-output` publishes, and the shape documents a
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
        fossil_descriptors_output::PROVIDERS,
    ));
    let mut db = FossilDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), program_path.to_string());
    for (path, text) in documents {
        let doc = SourceFile::new(&db, (*text).to_string(), (*path).to_string());
        register_file(&mut db, (*path).to_string(), doc);
    }
    (db, file)
}

/// Every diagnostic the file's mappings accumulate, in mapping order.
///
/// A mapping that does not compile makes `execute_*` return
/// `Plan("the mapping did not compile; see the reported diagnostics")` — a
/// message that names no diagnostic. This is how a test says which.
pub(crate) fn diagnostics(db: &FossilDb, file: SourceFile) -> Vec<String> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mut out = Vec::new();
    for mapping in def_map.mappings(db) {
        let _ = fossil_mir::lower_to_mir_pg(db, *mapping);
        out.extend(
            fossil_mir::lower_to_mir_pg::accumulated::<fossil_base::Diagnostic>(db, *mapping)
                .into_iter()
                .map(fossil_base::Diagnostic::message),
        );
    }
    out
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

/// Where [`write_in_memory`] writes.
pub(crate) const MEMORY_DEST: &str = "mem://out/corpus/";

/// [`fossil_df::write`] into an in-memory store, and every file it wrote, by
/// path under [`MEMORY_DEST`].
pub(crate) async fn write_in_memory(
    graph: &fossil_df::Graph,
) -> (
    fossil_df::Written,
    std::collections::BTreeMap<String, Vec<u8>>,
) {
    use futures::TryStreamExt;
    use object_store::{ObjectStore, ObjectStoreExt};
    let mut storage = fossil_storage::Storage::new(Arc::new(NoHost));
    let out = Arc::new(object_store::memory::InMemory::new());
    storage
        .with_store("mem://out/", Arc::clone(&out) as Arc<dyn ObjectStore>)
        .expect("route the in-memory store");
    let written = fossil_df::write(graph, &storage, MEMORY_DEST)
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
            .strip_prefix("corpus/")
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
    descriptor: &fossil_df::OutputDescriptorKind,
) {
    let none = std::collections::HashMap::new();
    for binding in fossil_df::provider_bindings(db, file, descriptor, &none) {
        let turtle = std::fs::read_to_string(&binding.uri)
            .unwrap_or_else(|e| panic!("read RDF source `{}`: {e}", binding.uri));
        fossil_df::register_rdf(ctx, &binding, &turtle).expect("register the RDF source");
    }
}
