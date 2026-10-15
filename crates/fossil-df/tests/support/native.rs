//! **The native test host** — what a host does around the compiler, for the
//! tests that need a whole host and not a hand-built database.
//!
//! Two halves, and they are two hosts on purpose:
//!
//! - **check** — a [`System`] over the real filesystem, with the whole provider
//!   registry (`fossil_hir::providers::PROVIDERS`), the sources
//!   `profile::profile_and_register` describes before the compile,
//!   and every shape document the program names registered from disk. It is the
//!   editor's view of a program: forward-propagated source types, every
//!   diagnostic, and [`render_diagnostics`] drawing them as a terminal host
//!   would.
//! - **run** — [`fossil_df::Executor`], the one host that writes a graph. The
//!   program's directory is loaded into an in-memory store under [`AUTHORITY`],
//!   the program is compiled AT its URL there, so every relative source and
//!   document resolves beside it as it would for any reader of that file, and the
//!   graph is written into memory (`Output::Memory`). The executor reads no file
//!   and introspects nothing; what the checker sees and what the run sees differ
//!   exactly as the browser's two workspaces do.

// These items are `pub(crate)` (a private module ⇒ `unreachable_pub` wants
// `pub(crate)`), which trips the inverse `redundant_pub_crate` nursery lint.
#![allow(clippy::redundant_pub_crate)]
// Shared by several test binaries and no binary uses all of it.
#![allow(dead_code)]
// The executor is single-threaded, so its futures are not `Send` and need not be.
#![allow(clippy::future_not_send)]

#[path = "profile.rs"]
pub(crate) mod profile;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{Diagnostic, FossilDb, SourceFile, System, file_at};
use fossil_df::{Executor, Output, Ran, RunReport};
use fossil_hir::documents::{documents_named, register_missing_documents, registry_key};
use graphar_info::{EdgeInfo, GraphInfo, VertexInfo};
use graphar_parquet::paths;
use miette::{GraphicalReportHandler, GraphicalTheme, LabeledSpan, NamedSource, SourceSpan};
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};

// ───────────────────────────────────────────────────────────────────── check

/// The host every check here runs on: every row of the provider registry, the
/// types-reading ones included.
pub(crate) fn check_host() -> NativeSystem {
    NativeSystem::with_providers(fossil_hir::providers::PROVIDERS)
}

/// A database over a fresh [`check_host`] holding the program at `path`: its
/// sources introspected first, then every shape document it names read from
/// disk and registered — parse → which documents → read → register → check,
/// the order the registry forces.
pub(crate) fn open_db(path: &Path) -> (FossilDb, SourceFile) {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let system: Arc<dyn System> = Arc::new(check_host());
    let mut db = FossilDb::new(system);
    let file = SourceFile::new(&db, text, path.to_string_lossy().into_owned());
    // A source that cannot be described is skipped, not fatal: the compile then
    // has no forward-propagated type for it, which is what the checker reports.
    let inputs = fossil_hir::lineage::inputs(&db, file);
    profile::profile_and_register(&mut db, &inputs);
    register_shape_documents(&mut db, file);
    (db, file)
}

/// Read and register every shape document `file` names. One that cannot be
/// read is skipped: the checker has the span of the `io.shex("…")` that named
/// it, and says so.
pub(crate) fn register_shape_documents(db: &mut FossilDb, file: SourceFile) {
    register_missing_documents(db, file, &|db, location| {
        db.system()
            .read_file(Path::new(location))
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    });
}

/// What a check of one program answers.
#[derive(Debug)]
pub(crate) struct CheckOutcome {
    pub(crate) source: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// The shape documents the program names, under the program's spelling,
    /// with their text — a diagnostic can point into one.
    pub(crate) documents: Vec<(String, String)>,
    /// How many mappings the file defines.
    pub(crate) mappings: usize,
}

/// Check the program at `path` against the whole native host.
pub(crate) fn check(path: &Path) -> CheckOutcome {
    let (db, file) = open_db(path);
    let documents = documents_named(&db, file)
        .into_iter()
        .filter_map(|document| {
            let key = registry_key(&db, file, &document);
            let text = file_at(&db, &key)?.text(&db).clone();
            Some((document.to_string(), text))
        })
        .collect();
    CheckOutcome {
        source: file.text(&db).clone(),
        diagnostics: fossil_mir::program_diagnostics(&db, file),
        documents,
        mappings: fossil_hir::def_map::def_map(&db, file).mappings(&db).len(),
    }
}

/// A diagnostic in the shape miette renders: its code, labels and an optional
/// `help`.
#[derive(Debug)]
struct Rendered {
    code: &'static str,
    message: String,
    src: NamedSource<String>,
    labels: Vec<LabeledSpan>,
    help: Option<String>,
}

impl std::fmt::Display for Rendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Rendered {}

impl miette::Diagnostic for Rendered {
    fn code(&self) -> Option<Box<dyn std::fmt::Display + '_>> {
        Some(Box::new(self.code))
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.src)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(self.labels.clone().into_iter()))
    }

    fn help(&self) -> Option<Box<dyn std::fmt::Display + '_>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h) as Box<dyn std::fmt::Display>)
    }
}

/// Render every diagnostic as a terminal host renders it, into one stable
/// string: miette graphical, unicode, no colour, width 100.
///
/// The `NamedSource` carries the program's FILE NAME and not its path — an
/// artefact with an absolute path in it only holds on one machine. The header
/// carries the problem's code; the `help:` line is the diagnostic's own prose,
/// else its structured suggestion.
///
/// # The severity is not in the artefact
///
/// Every diagnostic a program can produce is an error, and miette draws `×` for
/// one. The day an emitter raises a warning, [`Rendered`] implements
/// `miette::Diagnostic::severity`, or the artefact calls a warning an error.
pub(crate) fn render_diagnostics(
    source: &str,
    file_name: &str,
    diagnostics: &[Diagnostic],
    documents: &[(String, String)],
) -> String {
    let handler = GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
        .with_width(100)
        .with_context_lines(1);
    let named = NamedSource::new(file_name, source.to_string());
    let mut out = String::new();
    let at = |span: fossil_graph_schema::Span, text: &str| {
        LabeledSpan::new_with_span(
            Some(text.to_string()),
            SourceSpan::new(
                (span.start as usize).into(),
                span.end.saturating_sub(span.start) as usize,
            ),
        )
    };
    for d in diagnostics {
        let help = d.help.clone().or_else(|| d.suggestion_source.clone());
        let labels = if d.labels.is_empty() {
            vec![at(d.span, "here")]
        } else {
            d.labels
                .iter()
                .filter(|l| l.document.is_none())
                .map(|l| at(l.span, &l.text))
                .collect()
        };
        let rendered = Rendered {
            code: d.problem.code(),
            message: d.message(),
            src: named.clone(),
            labels,
            help,
        };
        let _ = handler.render_report(&mut out, &rendered);
        // **The other half of a two-file report.** miette resolves every range
        // against the one source its report carries, so a label in `shape.shex`
        // is a SECOND report over that document's text. Rendering it against the
        // program would underline whatever sits at that byte, silently. A label
        // naming a document that could not be read is dropped, which is what the
        // checker saw too.
        for (name, text) in documents {
            let in_this: Vec<LabeledSpan> = d
                .labels
                .iter()
                .filter(|l| l.document.as_deref() == Some(name.as_str()))
                .map(|l| at(l.span, &l.text))
                .collect();
            if in_this.is_empty() {
                continue;
            }
            let rendered = Rendered {
                code: d.problem.code(),
                message: d.message(),
                src: NamedSource::new(name, text.clone()),
                labels: in_this,
                // The `help:` rode on the program's snippet; one repair said
                // twice is not two repairs.
                help: None,
            };
            let _ = handler.render_report(&mut out, &rendered);
        }
        out.push('\n');
    }
    out
}

// ─────────────────────────────────────────────────────────────────────── run

/// The authority a program's directory is served under. Synthetic: nothing
/// resolves it but the in-memory store routed at it.
pub(crate) const AUTHORITY: &str = "https://programs.test/";

/// What a run answered and every file it wrote, by path under its root.
#[derive(Debug)]
pub(crate) struct Written {
    pub(crate) report: RunReport,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
}

impl Written {
    fn read(&self) -> impl Fn(&str) -> Option<Vec<u8>> + '_ {
        |path| self.files.get(path).cloned()
    }

    /// `fossil.graph.yml` and the documents it names, parsed into `GraphAr`'s own
    /// model by `graphar-info`.
    pub(crate) fn graph(&self) -> GraphInfo {
        crate::support::graph(&self.read())
    }

    /// The vertex type `ty`.
    pub(crate) fn vertex(&self, ty: &str) -> VertexInfo {
        self.graph()
            .vertices
            .into_iter()
            .find(|v| v.r#type == ty)
            .unwrap_or_else(|| panic!("the graph has no vertex type `{ty}`"))
    }

    /// The edge type `Src_label_Dst`.
    pub(crate) fn edge(&self, triple: &str) -> EdgeInfo {
        self.graph()
            .edges
            .into_iter()
            .find(|e| e.triple() == triple)
            .unwrap_or_else(|| panic!("the graph has no edge type `{triple}`"))
    }

    /// The names of the vertex types, in the order the graph lists them.
    pub(crate) fn vertex_types(&self) -> Vec<String> {
        self.graph()
            .vertices
            .into_iter()
            .map(|v| v.r#type)
            .collect()
    }

    /// The names of the edge types, in the order the graph lists them.
    pub(crate) fn edge_types(&self) -> Vec<String> {
        self.graph().edges.iter().map(EdgeInfo::triple).collect()
    }

    /// The vertices of `ty`, from its `vertex_count` file.
    pub(crate) fn vertex_count(&self, ty: &str) -> u64 {
        crate::support::vertex_count(&self.read(), &self.graph(), ty)
    }

    /// The edges of `triple`, from its `edge_count` files.
    pub(crate) fn edge_count(&self, triple: &str) -> u64 {
        crate::support::edge_count(&self.read(), &self.graph(), triple)
    }

    /// The path of the one chunk of `ty`'s one property group.
    pub(crate) fn chunk(&self, ty: &str) -> String {
        let info = self.vertex(ty);
        paths::vertex_chunk(&info, &info.property_groups[0], 0)
    }

    /// The path of the one chunk of `triple`'s adjacency list.
    pub(crate) fn adjacency(&self, triple: &str) -> String {
        let info = self.edge(triple);
        paths::adj_list_chunk(&info, &info.adj_lists[0], 0, 0)
    }

    /// The `(source subject, destination subject)` pairs of `triple`, read back
    /// through the indices by `DuckDB`, sorted.
    pub(crate) fn pairs(&self, root: &Path, triple: &str) -> Vec<(String, String)> {
        let info = self.edge(triple);
        let conn = duckdb::Connection::open_in_memory().expect("duckdb");
        let at = |relative: String| {
            root.join(relative)
                .display()
                .to_string()
                .replace('\'', "''")
        };
        let mut stmt = conn
            .prepare(&format!(
                "SELECT s.subject, d.subject FROM read_parquet('{e}') e \
                 JOIN read_parquet('{s}') s ON s._graphArVertexIndex = e._graphArSrcIndex \
                 JOIN read_parquet('{d}') d ON d._graphArVertexIndex = e._graphArDstIndex \
                 ORDER BY 1, 2",
                e = at(self.adjacency(triple)),
                s = at(self.chunk(&info.src_type)),
                d = at(self.chunk(&info.dst_type)),
            ))
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    /// The graph copied into a fresh directory, so `DuckDB` can read it.
    pub(crate) fn materialise(&self) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, bytes) in &self.files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(path, bytes).expect("write");
        }
        dir
    }
}

/// Every file under `dir`, by path relative to it.
fn files_under(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(at: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
        for entry in std::fs::read_dir(at).unwrap_or_else(|e| panic!("read {}: {e}", at.display()))
        {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .expect("under the base")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

/// Run the program `dir/program` through [`Executor`], with `connections`
/// (`name → URL`, usually under [`AUTHORITY`]) for its `@conn` references.
///
/// Every file under `dir` is served at [`AUTHORITY`], the program is compiled
/// at its URL there, and each document the executor reports missing is read
/// from `dir` by its location. Blocks on a current-thread runtime: the executor
/// spawns nothing, so one thread is the browser's situation.
///
/// # Errors
/// The executor's refusal, as its message.
pub(crate) fn run_dir(
    dir: &Path,
    program: &str,
    connections: &[(&str, &str)],
) -> Result<Written, String> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
        .block_on(run_dir_async(dir, program, connections))
}

async fn run_dir_async(
    dir: &Path,
    program: &str,
    connections: &[(&str, &str)],
) -> Result<Written, String> {
    let text = std::fs::read_to_string(dir.join(program))
        .unwrap_or_else(|e| panic!("read {program}: {e}"));

    let mut storage = fossil_storage::Storage::new(Arc::new(crate::support::NoHost));
    let served = Arc::new(InMemory::new());
    for (rel, path) in files_under(dir) {
        let bytes = std::fs::read(&path).expect("read a served file");
        served
            .put(&object_store::path::Path::from(rel), bytes.into())
            .await
            .expect("serve");
    }
    storage
        .with_store(AUTHORITY, Arc::clone(&served) as Arc<dyn ObjectStore>)
        .expect("route the program's directory");
    let mut exec = Executor::at(&text, &format!("{AUTHORITY}{program}"));
    exec.set_connections(
        connections
            .iter()
            .map(|(name, url)| ((*name).to_string(), (*url).to_string()))
            .collect(),
    );
    for missing in exec.missing_documents() {
        let rel = missing
            .location
            .strip_prefix(AUTHORITY)
            .unwrap_or_else(|| panic!("`{}` is not served here", missing.location));
        // A document that is not there stays unregistered, and the run reports
        // it as the browser would.
        if let Ok(text) = std::fs::read_to_string(dir.join(rel)) {
            exec.register_document(&missing.key, &text);
        }
    }

    let Ran::Memory { report, files } = exec
        .execute(&mut storage, &Output::Memory, None)
        .await
        .map_err(|e| {
            // The message and every cause under it: «the query engine failed»
            // alone says nothing a reader can act on.
            let mut text = e.to_string();
            let mut cause = std::error::Error::source(&e);
            while let Some(c) = cause {
                text = format!("{text}: {c}");
                cause = c.source();
            }
            text
        })?
    else {
        unreachable!("a run into memory answers its files")
    };
    let files = files
        .into_iter()
        .map(|file| (file.path, file.bytes.to_vec()))
        .collect();
    Ok(Written { report, files })
}

/// A fresh directory holding each `(relative path, text)`.
pub(crate) fn write_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, text) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("write {rel}: {e}"));
    }
    dir
}

/// One column of one table of a materialised graph, read back by `DuckDB` and
/// sorted — never through the writer's own types.
pub(crate) fn column(root: &Path, table: &str, column: &str) -> Vec<String> {
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let path = root.join(table).display().to_string().replace('\'', "''");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT CAST({column} AS VARCHAR) AS v FROM read_parquet('{path}') ORDER BY v"
        ))
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

/// `root/relative`, as a `DuckDB` string literal.
pub(crate) fn at(root: &Path, relative: &str) -> String {
    format!(
        "'{}'",
        root.join(relative)
            .display()
            .to_string()
            .replace('\'', "''")
    )
}

/// The rows `sql` answers on `DuckDB`, each value as text and `None` for NULL:
/// the caller casts what is not text.
pub(crate) fn rows(sql: &str) -> Vec<Vec<Option<String>>> {
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let mut result = stmt.query([]).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let width = result.as_ref().expect("a statement").column_count();
    let mut out = Vec::new();
    while let Some(row) = result.next().unwrap_or_else(|e| panic!("{sql}: {e}")) {
        out.push(
            (0..width)
                .map(|i| row.get::<_, Option<String>>(i).expect("a text cell"))
                .collect(),
        );
    }
    out
}
