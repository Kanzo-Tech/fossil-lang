//! **Every path the manifest names is on disk after `run` returns.**
//!
//! The corpus is read by a host with nothing to re-introspect with: it fetches
//! `graph.graph.yml`, follows the per-type documents it names, and computes each
//! tile's URL from `prefix` + `chunk_size` + the row count. Nothing is
//! discovered by listing, so every one of those strings owes its existence.
//!
//! It did not. `run` used to build a `RunStatus` from the executor's own view
//! (`vertex/<Type>.parquet` — what `run_to_dir` writes) and then run the W3
//! layout pass, whose LAST act is to delete exactly those files after tiling
//! them into `vertex/<Type>/chunk{k}.parquet`. Nothing failed: the deletion
//! succeeded, the status serialised, the CLI's own assertion
//! (`first["file"].starts_with("vertex/")`) held on a string, and the host got a
//! 404 it had no way to attribute. The status is gone and the report is the
//! manifest, which has always declared the chunk prefix — so the defect is not
//! reachable by construction any more. This file is what says so out loud, and
//! what would notice if some future pass moved a file the manifest names.
//!
//! # What this cannot prove
//!
//! Edge tiles are not asserted. A tile with no rows is not written, so a missing
//! `<edge prefix>by_source/tile{k}.parquet` is the answer «no edges in this
//! tile» and not a hole — which means existence is the wrong predicate for them
//! and only summing them against `edge_count` is the right one.
//! `apps/corpus`'s `declared-count` guard is where that is done, over a corpus
//! this crate does not write.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use fossil_sinks::manifest::VertexInfo;

const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")
User := io.csv(\"users.csv\")
People : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name     = User.name
";

const DOCUMENT: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string
}
";

const USERS: &str = "id,name\n1,Alice\n2,Bob\n3,Cleo\n";

/// Every tile of one vertex type, addressed the way a reader addresses them:
/// `ceil(vertex_count / chunk_size)` files named `chunk{k}.parquet` under the
/// declared `prefix`. A reader computes this list before it emits a request, so
/// a name it computes and cannot fetch is the failure.
fn tiles(info: &VertexInfo) -> Vec<String> {
    (0..info.vertex_count.div_ceil(info.chunk_size))
        .map(|k| format!("{}chunk{k}.parquet", info.prefix))
        .collect()
}

/// Run the fixture program into a fresh dest and hand back both.
fn run(dir: &tempfile::TempDir) -> (std::path::PathBuf, fossil_df::RunReport) {
    std::fs::write(dir.path().join("prog.fossil"), PROGRAM).expect("write program");
    std::fs::write(dir.path().join("person.shex"), DOCUMENT).expect("write document");
    std::fs::write(dir.path().join("users.csv"), USERS).expect("write csv");

    let dest = dir.path().join("out");
    introspect(&dir.path().join("prog.fossil"));
    let report = fossil_engine::run(
        &dir.path().join("prog.fossil"),
        &dest.to_string_lossy(),
        &std::collections::HashMap::new(),
        None,
    )
    .expect("the walking-skeleton shape of program runs");
    (dest, report)
}

#[test]
fn every_path_the_manifest_names_exists_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (dest, report) = run(&dir);

    assert_eq!(report.vertices.len(), 1, "one shape, one vertex type");
    assert_eq!(report.vertices[0].vertex_count, 3);

    // The entry point, the per-type documents it names, and the payload those
    // name — all three, because a reader follows each to reach the next, and
    // because these are the BYTES: `run` wrote them to `dest` and the layout
    // pass then rewrote and deleted files under it. A struct agreeing with
    // itself is what the deleted `RunStatus` also did.
    let mut named = vec!["graph.graph.yml".to_string()];
    named.extend(report.graph.vertices.clone());
    named.extend(report.graph.edges.clone());
    named.extend(report.vertices.iter().flat_map(tiles));
    assert!(named.len() > 1, "the manifest named nothing to fetch");
    for rel in named {
        assert!(
            dest.join(&rel).exists(),
            "`{rel}` is what the manifest tells a reader to fetch, and it is not there"
        );
    }
}

/// And the specific shape of it: the vertices live under the chunk PREFIX the
/// manifest declares, and the staged single-file Parquet the layout pass
/// consumed is gone. Both halves, because «the tiles exist» would also pass if
/// the pass had simply stopped deleting — which is the other fix, and the one
/// that leaves a stale second copy of every vertex behind.
#[test]
fn the_vertices_are_the_chunk_prefix_and_the_staged_parquet_is_gone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (dest, report) = run(&dir);

    let vertex = &report.vertices[0];
    assert_eq!(vertex.vertex_type, "Person");
    assert_eq!(
        vertex.prefix, "vertex/Person/",
        "the chunk prefix, which is the string the layout pass tiles into"
    );
    assert!(
        !Path::new(&dest.join("vertex/Person.parquet")).exists(),
        "the staged single-file Parquet is the layout pass's input and it is deleted"
    );
    let chunks: Vec<_> = std::fs::read_dir(dest.join(&vertex.prefix))
        .expect("the prefix is a directory")
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".parquet"))
        .collect();
    assert_eq!(
        chunks.len(),
        tiles(vertex).len(),
        "the prefix holds the tiles the count implies, and no others"
    );
}

/// Introspect before compiling — what `fossil-cli` does, and what `check`/`run`
/// stopped doing for themselves. Without it a program's sources have no
/// forward-propagated types, which is a different (and quietly weaker) answer.
fn introspect(path: &std::path::Path) {
    let system = fossil_engine::host_system(path);
    let _ = fossil_introspect::introspect_program(
        &*system,
        path,
        &std::collections::HashMap::new(),
        &fossil_introspect::RunCreds::default(),
    );
}
