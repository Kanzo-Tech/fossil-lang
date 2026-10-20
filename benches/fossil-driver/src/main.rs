//! **The benchmark's fossil host**: one program through [`fossil_df::Executor`],
//! timed, for `bench/run.py` to compare with the RML processors.
//!
//! ```text
//! cargo run --release --manifest-path benches/fossil-driver/Cargo.toml -- <dir> <program.fossil> <out> [--triples-sql <file.sql>]
//! ```
//!
//! It is the test host of `tests/support/native.rs` and not a second one: every
//! file under `<dir>` is served from memory at a synthetic authority, the program
//! is compiled at its URL there, the graph is written into memory
//! (`Output::Memory`) and then copied to `<out>`. That is the browser's
//! situation — the executor reads no file — so the time to read the inputs is
//! reported apart from the run, as `serve_ms`.
//!
//! One JSON line on stdout: `serve_ms`, `run_ms` (compile, every query, the
//! Parquet encoding), `write_ms` (the files onto disk), `files` and `bytes`.
//!
//! `--triples-sql` then writes, after the clock, the `DuckDB` script that reads
//! the written graph back as RDF: a view over each table's Parquet, and the
//! view `rml::sql` renders from the `mapping.ttl` the run wrote — Statement R as
//! `tests/mapping.rs` holds it. The runner executes it on its own `DuckDB`,
//! which has Parquet built in, so the comparison with the RML processors'
//! N-Triples needs no extension download.

// The executor is single-threaded, so its futures are not `Send` and need not be.
#![allow(clippy::future_not_send)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use fossil_df::{Executor, Output, Ran};
use graphar_parquet::paths;
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};

/// The authority the program's directory is served under, as the tests serve it.
const AUTHORITY: &str = "https://programs.test/";

/// A host that vends nothing: the run writes into memory and reads only what
/// is served.
#[derive(Debug)]
struct NoHost;

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
            Err(fossil_graph_schema::Foreign::named("Error", "the benchmark vends nothing").into())
        })
    }
}

fn files_under(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(at: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
        for entry in std::fs::read_dir(at).unwrap_or_else(|e| panic!("read {}: {e}", at.display()))
        {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path.strip_prefix(base).expect("under the base");
                out.push((rel.to_string_lossy().replace('\\', "/"), path));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

async fn run(dir: &Path, program: &str, out: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(dir.join(program)).map_err(|e| format!("{program}: {e}"))?;

    let started = Instant::now();
    let mut storage = fossil_storage::Storage::new(Arc::new(NoHost));
    let served = Arc::new(InMemory::new());
    for (rel, path) in files_under(dir) {
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        served
            .put(&object_store::path::Path::from(rel), bytes.into())
            .await
            .map_err(|e| e.to_string())?;
    }
    storage
        .with_store(AUTHORITY, Arc::clone(&served) as Arc<dyn ObjectStore>)
        .map_err(|e| e.to_string())?;
    let serve = started.elapsed();

    let started = Instant::now();
    let mut exec = Executor::at(&text, &format!("{AUTHORITY}{program}"));
    for missing in exec.missing_documents() {
        let rel = missing
            .location
            .strip_prefix(AUTHORITY)
            .ok_or_else(|| format!("`{}` is not served here", missing.location))?;
        let text = std::fs::read_to_string(dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        exec.register_document(&missing.key, &text);
    }
    let ran = exec
        .execute(&mut storage, &Output::Memory, None)
        .await
        .map_err(|e| {
            let mut text = format!("{e:?}");
            let mut cause = std::error::Error::source(&e);
            while let Some(c) = cause {
                text = format!("{text}: {c}");
                cause = c.source();
            }
            text
        })?;
    let Ran::Memory { files, .. } = ran else {
        unreachable!("a run into memory answers its files")
    };
    let run = started.elapsed();

    let started = Instant::now();
    let mut bytes = 0;
    for file in &files {
        let path = out.join(&file.path);
        std::fs::create_dir_all(path.parent().expect("a parent")).map_err(|e| e.to_string())?;
        std::fs::write(&path, &file.bytes).map_err(|e| e.to_string())?;
        bytes += file.bytes.len();
    }
    let write = started.elapsed();

    println!(
        "{{\"serve_ms\": {:.3}, \"run_ms\": {:.3}, \"write_ms\": {:.3}, \"files\": {}, \"bytes\": {bytes}}}",
        serve.as_secs_f64() * 1e3,
        run.as_secs_f64() * 1e3,
        write.as_secs_f64() * 1e3,
        files.len(),
    );
    Ok(())
}

/// Every chunk of a table: `…/chunk0` becomes `…/chunk*`, and an adjacency
/// list's `part0` becomes `part*`.
fn chunks(root: &Path, first: &str) -> String {
    let glob = first
        .strip_suffix("chunk0")
        .map(|stem| format!("{stem}chunk*"))
        .expect("a first chunk")
        .replace("/part0/", "/part*/");
    format!(
        "'{}'",
        root.join(glob).display().to_string().replace('\'', "''")
    )
}

/// The `DuckDB` script that reads the graph under `root` back as RDF: a view
/// per table over its Parquet chunks, then `triples`, the view `rml::sql`
/// renders from the written `mapping.ttl`.
fn triples_sql(root: &Path) -> Result<String, String> {
    let read =
        |name: &str| std::fs::read_to_string(root.join(name)).map_err(|e| format!("{name}: {e}"));
    let graph = graphar_info::GraphInfo::load(&read("fossil.graph.yml")?, |name| {
        Ok(read(name).unwrap_or_else(|e| panic!("{e}")))
    })
    .map_err(|e| e.to_string())?;
    let mapping = rml::rdf::parse_turtle(&read("mapping.ttl")?, None).map_err(|e| e.to_string())?;
    let mut sql = String::new();
    for vertex in &graph.vertices {
        if vertex.property_groups.len() != 1 {
            return Err(format!(
                "`{}` has {} property groups",
                vertex.r#type,
                vertex.property_groups.len()
            ));
        }
        let glob = chunks(
            root,
            &paths::vertex_chunk(vertex, &vertex.property_groups[0], 0),
        );
        let _ = writeln!(
            sql,
            "CREATE VIEW \"{}\" AS SELECT * FROM read_parquet({glob});",
            vertex.r#type
        );
    }
    for edge in &graph.edges {
        let glob = chunks(root, &paths::adj_list_chunk(edge, &edge.adj_lists[0], 0, 0));
        let _ = writeln!(
            sql,
            "CREATE VIEW \"{}\" AS SELECT * FROM read_parquet({glob});",
            edge.triple()
        );
    }
    let create = rml::sql::create_view(&mapping, None, "triples", |t| Ok(format!("\"{t}\"")))
        .map_err(|e| e.to_string())?;
    let _ = writeln!(sql, "{create};");
    Ok(sql)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (dir, program, out) = match args.as_slice() {
        [dir, program, out, ..] => (Path::new(dir), program.as_str(), Path::new(out)),
        _ => {
            eprintln!("usage: bench <dir> <program.fossil> <out> [--triples-sql <file.sql>]");
            std::process::exit(2);
        }
    };
    let ran = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
        .block_on(run(dir, program, out));
    if let Err(e) = ran {
        eprintln!("{e}");
        std::process::exit(1);
    }
    if let [_, _, _, flag, file] = args.as_slice() {
        assert_eq!(
            flag, "--triples-sql",
            "the one option is --triples-sql <file.sql>"
        );
        match triples_sql(out) {
            Ok(sql) => std::fs::write(file, sql).expect("write the script"),
            Err(e) => {
                eprintln!("triples: {e}");
                std::process::exit(1);
            }
        }
    }
}
