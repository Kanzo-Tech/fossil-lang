//! **Every path a [`RunStatus`] hands a host names something that exists.**
//!
//! The wire contract is read by a host that has no `DuckDB` to re-introspect
//! with: it takes `vertices[].file` and fetches it. So the one thing the field
//! owes is that it is there after `run` returns — and it was not. `run` builds
//! the status from the executor's own view (`vertex/<Type>.parquet`, which is
//! what `run_to_dir` writes and what the browser executor still emits) and then
//! runs the W3 layout pass, whose LAST act is to delete exactly those files
//! after tiling them into `vertex/<Type>/chunk{k}.parquet`.
//!
//! Nothing failed. The deletion succeeded, the status serialised, the CLI's own
//! assertion (`first["file"].starts_with("vertex/")`) held on a string, and the
//! host got a 404 it had no way to attribute. A docblock cannot hold this — the
//! two halves are in two crates and neither is wrong on its own — so what holds
//! it is a `run` that goes to disk and an `assert!(exists)` per path.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

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

/// Every path the status hands out, resolved against the dataset root.
fn paths(status: &fossil_run_status::RunStatus) -> Vec<String> {
    let mut out: Vec<String> = status.vertices.iter().map(|v| v.file.clone()).collect();
    for e in &status.edges {
        out.push(e.by_source.clone());
        out.push(e.by_target.clone());
    }
    out
}

#[test]
fn every_path_the_run_status_names_exists_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("prog.fossil"), PROGRAM).expect("write program");
    std::fs::write(dir.path().join("person.shex"), DOCUMENT).expect("write document");
    std::fs::write(dir.path().join("users.csv"), USERS).expect("write csv");

    let dest = dir.path().join("out");
    let status = fossil_engine::run(
        &dir.path().join("prog.fossil"),
        &dest.to_string_lossy(),
        &fossil_engine::RunCreds::default(),
        None,
    )
    .expect("the walking-skeleton shape of program runs");

    assert_eq!(status.vertices.len(), 1, "one shape, one vertex type");
    assert_eq!(status.vertices[0].count, Some(3));

    let named = paths(&status);
    assert!(!named.is_empty(), "the status named nothing to fetch");
    for rel in named {
        let path = dest.join(&rel);
        assert!(
            path.exists(),
            "`{rel}` is what the status tells a host to fetch, and it is not there"
        );
    }
}

/// And the specific shape of it: after the layout pass the vertices live under
/// the chunk PREFIX the `GraphAr` manifest already declares, and the staged
/// single-file Parquet the pass consumed is gone. Both halves, because
/// «the file exists» would also pass if the pass had simply stopped deleting —
/// which is the other fix, and the one that leaves a stale second copy of every
/// vertex behind.
#[test]
fn the_vertices_are_the_chunk_prefix_and_the_staged_parquet_is_gone() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("prog.fossil"), PROGRAM).expect("write program");
    std::fs::write(dir.path().join("person.shex"), DOCUMENT).expect("write document");
    std::fs::write(dir.path().join("users.csv"), USERS).expect("write csv");

    let dest = dir.path().join("out");
    let status = fossil_engine::run(
        &dir.path().join("prog.fossil"),
        &dest.to_string_lossy(),
        &fossil_engine::RunCreds::default(),
        None,
    )
    .expect("run");

    let vertex = &status.vertices[0];
    assert_eq!(vertex.vertex_type, "Person");
    assert_eq!(
        vertex.file, "vertex/Person/",
        "the chunk prefix, which is the same string `VertexInfo::prefix` declares"
    );
    assert!(
        !Path::new(&dest.join("vertex/Person.parquet")).exists(),
        "the staged single-file Parquet is the layout pass's input and it is deleted"
    );
    let chunks: Vec<_> = std::fs::read_dir(dest.join(&vertex.file))
        .expect("the prefix is a directory")
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".parquet"))
        .collect();
    assert!(
        !chunks.is_empty(),
        "and the prefix holds the chunks the rows moved into"
    );
}
