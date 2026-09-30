//! **The native test host** — what the deleted `fossil` binary did around the
//! compiler, kept for the tests that need a whole host and not a hand-built
//! database.
//!
//! Two halves, and they are two hosts on purpose:
//!
//! - **check** — a [`System`] over the real filesystem, with the whole provider
//!   registry (`fossil_descriptors_output::PROVIDERS`) and a descriptor cache
//!   `fossil_introspect::pre_introspect_and_register` fills before the compile,
//!   and every shape document the program names registered from disk. It is the
//!   editor's view of a program: forward-propagated source types, every
//!   diagnostic, and [`render_diagnostics`] drawing them as `fossil check` drew
//!   them.
//! - **run** — [`fossil_df::Executor`], the one host that writes a corpus. The
//!   program's directory is loaded into an in-memory store under [`AUTHORITY`],
//!   the program is compiled AT its URL there, so every relative source and
//!   document resolves beside it as it would for any reader of that file, and the
//!   corpus is written into a second in-memory store. The executor reads no file
//!   and introspects nothing; what the checker sees and what the run sees differ
//!   exactly as the browser's two workspaces do.

// These items are `pub(crate)` (a private module ⇒ `unreachable_pub` wants
// `pub(crate)`), which trips the inverse `redundant_pub_crate` nursery lint.
#![allow(clippy::redundant_pub_crate)]
// Shared by several test binaries and no binary uses all of it.
#![allow(dead_code)]
// The executor is single-threaded, so its futures are not `Send` and need not be.
#![allow(clippy::future_not_send)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::test_support::NativeSystem;
use fossil_base::{Diagnostic, FossilDb, FsError, Provider, SourceFile, System, file_at};
use fossil_descriptors_input::DescriptorCache;
use fossil_df::{Executor, RunReport};
use fossil_hir::documents::{documents_named, register_missing_documents, registry_key};
use fossil_sinks::manifest::Manifest;
use futures::TryStreamExt;
use miette::{GraphicalReportHandler, GraphicalTheme, LabeledSpan, NamedSource, SourceSpan};
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};

// ───────────────────────────────────────────────────────────────────── check

/// The real filesystem, the introspected-schema table, and every row of the
/// provider registry — the types-reading ones included.
#[derive(Debug, Default)]
pub(crate) struct CheckHost(NativeSystem);

impl System for CheckHost {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.0.read_file(path)
    }
    fn now(&self) -> SystemTime {
        self.0.now()
    }
    fn descriptors(&self) -> Option<&DescriptorCache> {
        self.0.descriptors()
    }
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}

/// A database over a fresh [`CheckHost`] holding the program at `path`: its
/// sources introspected first, then every shape document it names read from
/// disk and registered — parse → which documents → read → register → check,
/// the order the registry forces.
pub(crate) fn open_db(path: &Path) -> (FossilDb, SourceFile) {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let system: Arc<dyn System> = Arc::new(CheckHost::default());
    // A source that cannot be described is skipped, not fatal: the compile then
    // has no forward-propagated type for it, which is what the checker reports.
    let _ = fossil_introspect::introspect_program(
        Arc::clone(&system),
        path,
        &fossil_introspect::RunCreds::default(),
    );
    let mut db = FossilDb::new(system);
    let file = SourceFile::new(&db, text, path.to_string_lossy().into_owned());
    register_shape_documents(&mut db, file);
    (db, file)
}

/// Read and register every shape document `file` names. One that cannot be
/// read is skipped: the checker has the span of the `io.shex("…")` that named
/// it, and says so.
pub(crate) fn register_shape_documents(db: &mut FossilDb, file: SourceFile) {
    register_missing_documents(db, file, &|db, locator| {
        db.system()
            .read_file(Path::new(locator))
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

/// Every diagnostic message a check of `path` produces.
pub(crate) fn messages(path: &Path) -> Vec<String> {
    check(path)
        .diagnostics
        .into_iter()
        .map(|d| d.message)
        .collect()
}

/// A diagnostic in the shape miette renders: its labels and an optional `help`.
#[derive(Debug)]
struct Rendered {
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

/// Render every diagnostic as `fossil check` rendered it, into one stable
/// string: miette graphical, unicode, no colour, width 100.
///
/// The `NamedSource` carries the program's FILE NAME and not its path — an
/// artefact with an absolute path in it only holds on one machine. The `help:`
/// line is the diagnostic's own prose, else its structured suggestion, else an
/// inline «did you mean» clause lifted out of the message.
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
    let at = |span: fossil_base::Span, text: &str| {
        LabeledSpan::new_with_span(
            Some(text.to_string()),
            SourceSpan::new(
                (span.start as usize).into(),
                span.end.saturating_sub(span.start) as usize,
            ),
        )
    };
    for d in diagnostics {
        let help = d
            .help
            .clone()
            .or_else(|| d.suggestion_source.clone())
            .or_else(|| {
                d.message
                    .find("did you mean")
                    .map(|i| d.message[i..].to_string())
            });
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
            message: d.message.clone(),
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
                message: d.message.clone(),
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

/// Where a run writes.
pub(crate) const DEST: &str = "mem://out/corpus/";

/// What a run answered and every file it wrote, by path under [`DEST`].
#[derive(Debug)]
pub(crate) struct Corpus {
    pub(crate) report: RunReport,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
}

impl Corpus {
    /// `fossil.json`, parsed into the format's own structs.
    pub(crate) fn manifest(&self) -> Manifest {
        serde_json::from_slice(
            self.files
                .get("fossil.json")
                .expect("the run wrote fossil.json"),
        )
        .expect("fossil.json is the manifest")
    }

    /// The corpus copied into a fresh directory, so `DuckDB` can read it.
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
/// from `dir` by its locator. Blocks on a current-thread runtime: the executor
/// spawns nothing, so one thread is the browser's situation.
///
/// # Errors
/// The executor's refusal, as its message.
pub(crate) fn run_dir(
    dir: &Path,
    program: &str,
    connections: &[(&str, &str)],
) -> Result<Corpus, String> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
        .block_on(run_dir_async(dir, program, connections))
}

async fn run_dir_async(
    dir: &Path,
    program: &str,
    connections: &[(&str, &str)],
) -> Result<Corpus, String> {
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
    let out = Arc::new(InMemory::new());
    storage
        .with_store("mem://out/", Arc::clone(&out) as Arc<dyn ObjectStore>)
        .expect("route the destination");

    let mut exec = Executor::at(&text, &format!("{AUTHORITY}{program}"));
    exec.set_connections(
        connections
            .iter()
            .map(|(name, url)| ((*name).to_string(), (*url).to_string()))
            .collect::<HashMap<_, _>>(),
    );
    for missing in exec.missing_documents() {
        let rel = missing
            .locator
            .strip_prefix(AUTHORITY)
            .unwrap_or_else(|| panic!("`{}` is not served here", missing.locator));
        // A document that is not there stays unregistered, and the run reports
        // it as the browser would.
        if let Ok(text) = std::fs::read_to_string(dir.join(rel)) {
            exec.register_document(&missing.key, &text);
        }
    }

    let report = exec.execute(&mut storage, DEST).await?;
    let mut files = BTreeMap::new();
    let listed: Vec<_> = out.list(None).try_collect().await.expect("list");
    for meta in listed {
        let bytes = out
            .get(&meta.location)
            .await
            .expect("get")
            .bytes()
            .await
            .expect("bytes");
        let rel = meta
            .location
            .as_ref()
            .strip_prefix("corpus/")
            .expect("under the destination")
            .to_string();
        files.insert(rel, bytes.to_vec());
    }
    Ok(Corpus { report, files })
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

/// One column of one table of a materialised corpus, read back by `DuckDB` and
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
