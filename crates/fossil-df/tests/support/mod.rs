// These items are `pub(crate)` (a private module ⇒ `unreachable_pub` wants
// `pub(crate)`), which trips the inverse `redundant_pub_crate` nursery lint —
// silenced the way the rest of the workspace silences it.
#![allow(clippy::redundant_pub_crate)]

//! The host half these integration tests owe the checker: a filesystem, a
//! shape-decoder table, and the shape documents a program names, put into the
//! database before any query goes looking for them.
//!
//! Since ruling 3 of 2026-08-11 a property key is a BARE NAME
//! whose meaning is the last segment of a predicate IRI the shape declares, so
//! a program that names no shape document cannot write a single property — the
//! mapping stops compiling and `execute_*` fails with `Plan("the mapping did
//! not compile")`. Every program in this crate's tests therefore carries a
//! `type { … } := io.shex("…")` line, and every test builds its database here.
//!
//! `fossil_base::test_support::NativeSystem` is NOT enough on its own: its decoder table is
//! the trait default `&[]`, and a document nothing decodes resolves no shape —
//! which `resolve_target_shape` reports as `Undecodable`, not as a missing
//! document. Hence [`ShapeHost`], which delegates the filesystem to
//! `NativeSystem` and adds the one row `fossil-descriptors-output` publishes.
//! Same pair `fossil-lsp`'s `LspSystem` and `fossil-ide`'s hover test install.

// The module is shared by eight test binaries and no binary uses all of it.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, FsError, Provider, SourceFile, System, register_file};

/// The real filesystem (the CSV/JSON/Turtle sources still read through it) plus
/// the `ShEx` decoder row.
#[derive(Debug, Default)]
pub(crate) struct ShapeHost(NativeSystem);

impl System for ShapeHost {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.0.read_file(path)
    }
    fn now(&self) -> SystemTime {
        self.0.now()
    }
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}

/// A database over [`ShapeHost`] holding `program` at `program_path` and every
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
    let system: Arc<dyn System> = Arc::new(ShapeHost::default());
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
                .map(|d| d.message.clone()),
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
        Result<std::collections::HashMap<String, String>, fossil_graph_schema::Foreign>,
    > {
        Box::pin(async { Ok(std::collections::HashMap::new()) })
    }
    fn credentials(
        &self,
        _: &fossil_storage::Scope,
        _: fossil_storage::Access,
    ) -> futures::future::BoxFuture<
        'static,
        Result<Vec<fossil_storage::StorageCredential>, fossil_graph_schema::Foreign>,
    > {
        Box::pin(async {
            Err(fossil_graph_schema::Foreign::named(
                "Error",
                "these tests vend nothing",
            ))
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
