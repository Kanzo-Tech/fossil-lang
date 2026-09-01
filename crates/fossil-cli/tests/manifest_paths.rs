//! **Every path the manifest names is on disk after `run` returns.**
//!
//! The corpus is read by a host with nothing to re-introspect with: it fetches
//! `graph.graph.yml`, follows the per-type documents it names, and computes each
//! payload set's URL from `prefix` + the container the graph document declares.
//! Nothing is discovered by listing, so every one of those strings owes its
//! existence.
//!
//! It did not. `run` used to build a `RunStatus` from the executor's own view
//! (`vertex/<Type>.parquet` — what `run_to_dir` writes) and then run the W3
//! layout pass, whose LAST act is to delete exactly those files after tiling
//! them into `vertex/<Type>/`. Nothing failed: the deletion succeeded, the
//! status serialised, the CLI's own assertion
//! (`first["file"].starts_with("vertex/")`) held on a string, and the host got a
//! 404 it had no way to attribute. The status is gone and the report is the
//! manifest, which has always declared the tile prefix — so the defect is not
//! reachable by construction any more. This file is what says so out loud, and
//! what would notice if some future pass moved a file the manifest names.
//!
//! **A payload set is one file whose row groups are its tiles** since `818218c`,
//! so what the reader composes is `<prefix>tiles.parquet` and what addresses a
//! tile inside it is the row-group ordinal. That moves the count out of the
//! directory and into the footer: the tiles are still `ceil(vertex_count /
//! chunk_size)`, and asking the file how many row groups it has is the same
//! question the old `chunk{k}.parquet` listing asked of the directory.
//!
//! # What this cannot prove
//!
//! Edge tiles are not asserted. An adjacency tile holds whatever edges its
//! vertices happen to have, and one with none contributes no rows — so it gets
//! no row group, the ordinals are dense where the tile numbers are not, and
//! neither existence nor a count of row groups is a predicate arithmetic
//! predicts. Only summing them against `edge_count` is right.
//! `apps/corpus`'s `declared-count` guard is where that is done, over a corpus
//! this crate does not write.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use fossil_sinks::manifest::{TILE_CODES_FILE, TILES_FILE, VertexInfo};

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

/// The payload set of one vertex type, addressed the way a reader addresses it:
/// [`TILES_FILE`] under the declared `prefix`. A reader composes this string
/// before it emits a request, so a name it composes and cannot fetch is the
/// failure.
///
/// It was a `Vec` of `ceil(vertex_count / chunk_size)` names — one per tile,
/// each its own file. The tiles are row groups of this one file now, so the
/// arithmetic is not a list of URLs any more; it is [`row_groups`], asked of the
/// footer.
fn payload(info: &VertexInfo) -> String {
    format!("{}{TILES_FILE}", info.prefix)
}

/// The `codes:` block's `path`, by line scan, or `None` when the document
/// declares none.
///
/// A scan and not a deserialise, for the reason the whole file exists: reading
/// it back through `VertexInfo` would prove the struct round-trips, not that the
/// artefact tells a stranger where the anchor is. The shape it reads is the two
/// lines the writer emits — `codes:` then an indented `path:` — which is also
/// the shape `apps/corpus/guards/manifest.mjs` scans for.
fn codes_path(yaml: &str) -> Option<String> {
    let mut lines = yaml.lines().skip_while(|l| l.trim_end() != "codes:");
    lines.next()?;
    for line in lines {
        if !line.starts_with(' ') {
            return None;
        }
        if let Some(value) = line.trim().strip_prefix("path:") {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// How many tiles a payload set holds, read off its footer — one row group per
/// tile, which is what makes an ordinal an address.
fn row_groups(path: &Path) -> u64 {
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let count: i64 = conn
        .query_row(
            &format!(
                "SELECT count(DISTINCT row_group_id) FROM parquet_metadata('{}')",
                path.display().to_string().replace('\'', "''")
            ),
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| panic!("read the footer of {}: {e}", path.display()));
    u64::try_from(count).expect("a row-group count fits a u64")
}

/// Run the fixture program into a fresh dest and hand back both.
fn run(dir: &tempfile::TempDir) -> (std::path::PathBuf, fossil_df::RunReport) {
    std::fs::write(dir.path().join("prog.fossil"), PROGRAM).expect("write program");
    std::fs::write(dir.path().join("person.shex"), DOCUMENT).expect("write document");
    std::fs::write(dir.path().join("users.csv"), USERS).expect("write csv");

    let dest = dir.path().join("out");
    introspect(&dir.path().join("prog.fossil"));
    let report = fossil_cli::run(
        &dir.path().join("prog.fossil"),
        &dest.to_string_lossy(),
        &std::collections::HashMap::new(),
        None,
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
    named.extend(report.vertices.iter().map(payload));
    // And the tile-code anchor, read out of the vertex document rather than off
    // the report, because the report is the compile's view and the anchor is the
    // LAYOUT pass's: the manifest declares a path it has no numbers for and the
    // pass fills it afterwards. Exactly the shape the header of this file
    // describes — two sides agreeing about a string, and only one of them
    // writing bytes. If the pass stopped writing it, every reader that trusts
    // `codes:` gets a 404 and nothing else here would notice.
    for (info, doc) in report.vertices.iter().zip(&report.graph.vertices) {
        let yaml = std::fs::read_to_string(dest.join(doc)).expect("read the vertex document");
        let declared = codes_path(&yaml)
            .unwrap_or_else(|| panic!("`{doc}` declares no `codes:` path; got:\n{yaml}"));
        named.push(format!("{}{declared}", info.prefix));
    }
    assert!(named.len() > 1, "the manifest named nothing to fetch");
    for rel in named {
        assert!(
            dest.join(&rel).exists(),
            "`{rel}` is what the manifest tells a reader to fetch, and it is not there"
        );
    }

    // And the container that says how those strings are composed. It is the one
    // thing about a tile's URL a reader is told rather than derives, so a corpus
    // written with row groups and declaring `files` sends every reader to
    // `chunk0.parquet` — a 404 nothing above would catch, because every path
    // checked here is composed by the same side that wrote them.
    //
    // Read off the bytes and by line scan, for the reason the whole file exists:
    // a struct agreeing with itself is what the deleted `RunStatus` also did,
    // and deserialising through `GraphInfo` would prove that struct round-trips
    // rather than that the artefact says anything. The spelling is transcribed
    // because a stranger transcribes it too — `serde(rename_all = "lowercase")`
    // over `Container::RowGroups` is what puts it there.
    let graph_yaml =
        std::fs::read_to_string(dest.join("graph.graph.yml")).expect("read the graph document");
    assert!(
        graph_yaml
            .lines()
            .any(|line| line.trim_end() == "container: rowgroups"),
        "the graph document declares a container fossil does not write; got:\n{graph_yaml}"
    );
}

/// And the specific shape of it: the vertices live under the tile PREFIX the
/// manifest declares, and the staged single-file Parquet the layout pass
/// consumed is gone. Both halves, because «the tiles exist» would also pass if
/// the pass had simply stopped deleting — which is the other fix, and the one
/// that leaves a stale second copy of every vertex behind.
#[test]
fn the_vertices_are_the_tile_prefix_and_the_staged_parquet_is_gone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (dest, report) = run(&dir);

    let vertex = &report.vertices[0];
    assert_eq!(vertex.vertex_type, "Person");
    assert_eq!(
        vertex.prefix, "vertex/Person/",
        "the tile prefix, which is the string the layout pass tiles into"
    );
    assert!(
        !Path::new(&dest.join("vertex/Person.parquet")).exists(),
        "the staged single-file Parquet is the layout pass's input and it is deleted"
    );

    // ONE payload file under the prefix, beside the identity index, and nothing
    // else. It counted `.parquet` entries against the tile count, which is the
    // same question asked of the container that existed then: `chunk{k}` files
    // are gone, so the entry that must be there is `tiles.parquet` and a second
    // `.parquet` beside it is two containers for one set of rows.
    let mut emitted: Vec<String> = std::fs::read_dir(dest.join(&vertex.prefix))
        .expect("the prefix is a directory")
        .filter_map(std::result::Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    emitted.sort();
    assert_eq!(
        emitted,
        vec![
            TILE_CODES_FILE.to_string(),
            "index".to_string(),
            TILES_FILE.to_string()
        ],
        "the prefix holds the payload, the index and the code anchor, and nothing else"
    );

    // And the tiles are in the footer, which is where the count moved: a
    // directory listing answered it before, and a listing is the one thing the
    // corpus is designed so nobody performs. Derived from the declaration rather
    // than from the fixture, so it is the reader's own arithmetic.
    //
    // **Three rows at a `chunk_size` of 4,096 is one tile**, so what this can
    // catch is a payload written with no row groups at all or with more than the
    // count implies — not a writer that stopped cutting, because there is
    // nothing here to cut. `tests/conformance.rs` is where that is separated,
    // over ten thousand rows and three tiles, and it checks each tile's
    // `dense_id` range as well as the count.
    assert_eq!(
        row_groups(&dest.join(payload(vertex))),
        vertex.vertex_count.div_ceil(vertex.chunk_size),
        "the payload's row groups are not the tiles `vertex_count` and `chunk_size` imply"
    );
}

/// Introspect before compiling — what `fossil-cli` does, and what `check`/`run`
/// stopped doing for themselves. Without it a program's sources have no
/// forward-propagated types, which is a different (and quietly weaker) answer.
fn introspect(path: &std::path::Path) {
    let system = fossil_cli::host_system(path);
    let _ = fossil_introspect::introspect_program(
        &*system,
        path,
        &std::collections::HashMap::new(),
        &fossil_introspect::RunCreds::default(),
    );
}
