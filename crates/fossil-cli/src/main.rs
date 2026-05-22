//! `fossil` — the native CLI binary (`compile` / `check` / `run`).
//!
//! Three subcommands wire the full compiler+runtime pipeline end-to-end:
//!
//! - `fossil compile <FILE> [--out-dir DIR] [--shape FILE]` → `parse` →
//!   `def_map` → `lower_to_hir` → `lower_to_mir` → `codegen_sql` → execute via
//!   `DuckDB`; writes `output.parquet` and `manifest.yaml` (to `--out-dir`,
//!   default cwd) and prints the artifact paths (CLI-01).
//! - `fossil check <FILE> [--shape FILE]` → parse + `def_map` +
//!   `typecheck_mapping` over every mapping; drains the Salsa `Diagnostic`
//!   accumulator and renders each as a rustc-style miette report (source span,
//!   color, `help:` did-you-mean). Exits non-zero iff any `Severity::Error`
//!   diagnostic was accumulated (CLI-02 / SC#1).
//! - `fossil run <FILE> [--shape FILE]` → compile + execute via native `DuckDB`,
//!   then prints a result summary (triple/vertex/edge row counts) read back
//!   from the produced Parquet (CLI-03 / RESEARCH Pitfall 6).
//!
//! **WALKING-SKELETON INVARIANT:** the canonical demo
//! `fossil compile examples/hello.fossil` must keep succeeding end-to-end
//! through every compiler+runtime crate, byte-identically (5 triples).
//! Subsequent commits MUST NOT regress this — see ROADMAP.md sequencing rule
//! #6 + CLAUDE.md "Hard Rules".

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-cli is native-only (depends on fossil-runtime which uses bundled DuckDB); \
     do not add it to the WASM CI gate"
);

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use clap::{Parser, Subcommand};
use fossil_base::{Diagnostic, FsError, Severity, System};
use fossil_descriptors_output::{OutputDescriptorKind, SystemWithDescriptors};
use miette::{NamedSource, SourceSpan};
use tracing_subscriber::EnvFilter;

/// CLI host's [`System`] impl — a thin `NativeSystem`-style wrapper that
/// ALSO implements [`SystemWithDescriptors`] so the bidirectional checker
/// (plan 03-05) can reach the output-descriptor accessor via the extension
/// trait (Option B from plan 03-03 Task 2 step 3; see ADR-0006).
///
/// Phase 3 v0.1: `output_descriptor_kind()` returns the default
/// [`OutputDescriptorKind::ACCEPT_ALL_DEFAULT`]. Phase 6 LSP-01 / future CLI
/// flags override this to load a `ShEx` schema from a side file.
#[derive(Debug, Default)]
struct CliSystem;

impl System for CliSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound(path.display().to_string()),
            _ => FsError::Io(e.to_string()),
        })
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

impl SystemWithDescriptors for CliSystem {
    // Default impl returns AcceptAll — see SystemWithDescriptors. Phase 6
    // LSP-01 will override here to load a `ShEx` schema from a side file.
}

#[derive(Parser, Debug)]
#[command(
    name = "fossil",
    version,
    about = "Fossil — typed compiler for RDF graph construction",
    long_about = None,
)]
struct Cli {
    /// Increase log verbosity (overrides `RUST_LOG` default of WARN).
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Compile a `.fossil` file end-to-end: writes `output.parquet` and
    /// `manifest.yaml` (to `--out-dir`, default the current working directory).
    Compile {
        /// Path to the `.fossil` source file.
        file: PathBuf,
        /// Directory the `output.parquet` + `manifest.yaml` artifacts are
        /// written to. Defaults to the current working directory.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Path to a `ShEx` schema (`.shex`/JSON) used as the output target
        /// shape. If absent, a sibling `<stem>.shex` is auto-discovered;
        /// otherwise the checker stays `AcceptAll`.
        #[arg(long)]
        shape: Option<PathBuf>,
    },
    /// Parse + type-check a `.fossil` file, rendering rustc-style miette
    /// diagnostics. Exits 0 on a clean file; non-zero if any error diagnostic
    /// was accumulated.
    Check {
        /// Path to the `.fossil` source file.
        file: PathBuf,
        /// Path to a `ShEx` schema used as the output target shape (see
        /// `compile --shape`).
        #[arg(long)]
        shape: Option<PathBuf>,
    },
    /// Compile + execute a `.fossil` file via native `DuckDB`, then print a
    /// result summary (row counts).
    Run {
        /// Path to the `.fossil` source file.
        file: PathBuf,
        /// Path to a `ShEx` schema used as the output target shape (see
        /// `compile --shape`).
        #[arg(long)]
        shape: Option<PathBuf>,
    },
}

fn main() -> miette::Result<()> {
    miette::set_panic_hook();
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Commands::Check { file, shape } => cmd_check(&file, shape.as_deref()),
        Commands::Compile {
            file,
            out_dir,
            shape,
        } => cmd_compile(&file, out_dir.as_deref(), shape.as_deref()),
        Commands::Run { file, shape } => cmd_run(&file, shape.as_deref()),
    }
}

/// Initialise `tracing-subscriber` with `EnvFilter`. `RUST_LOG` overrides the
/// default; `--verbose` upgrades the implicit default from `warn` to `debug`
/// when no `RUST_LOG` is set. The canonical filter is `RUST_LOG=fossil=debug`
/// per CLAUDE.md "Style".
fn init_tracing(verbose: bool) {
    let default_directive = if verbose { "fossil=debug" } else { "warn" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_directive));
    // `try_init` so a second call (e.g. from a test harness) is a no-op rather
    // than a panic.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

/// A single type-check diagnostic, rendered rustc-style by miette's
/// `GraphicalReportHandler` (source span + caret art + color + `help:`).
///
/// Built from a drained [`fossil_base::Diagnostic`]: the byte span maps to a
/// [`SourceSpan`], and the did-you-mean / suggestion text becomes the `#[help]`
/// line. The `NamedSource` carries the whole source file so miette can render
/// the offending line with context.
#[derive(thiserror::Error, miette::Diagnostic, Debug)]
#[error("{message}")]
struct CheckError {
    message: String,
    #[source_code]
    src: NamedSource<String>,
    #[label("here")]
    span: SourceSpan,
    #[help]
    help: Option<String>,
}

/// Top-level aggregate that renders every per-mapping [`CheckError`] in one
/// rustc-style report (the `#[related]` list), then carries a terse summary as
/// its own `Display`.
#[derive(thiserror::Error, miette::Diagnostic, Debug)]
#[error("type-checking failed: {} error(s)", related.len())]
struct CheckReport {
    #[related]
    related: Vec<CheckError>,
}

/// Convert a drained [`Diagnostic`] into a [`CheckError`]. The `help` line is
/// the structured `suggestion_source` when present, otherwise `None` (the
/// did-you-mean text already lives inline in `message`, so miette renders it as
/// the headline). Spans are byte offsets, directly usable as a [`SourceSpan`].
///
/// `message` is the diagnostic text verbatim from the checker, which routes
/// every rendered type through `fossil_hir::render_ty_kind` — so a
/// `TyKind::Unknown` never reaches this layer (it renders as `?`).
fn to_check_error(d: &Diagnostic, src: &NamedSource<String>) -> CheckError {
    let len = d.span.end.saturating_sub(d.span.start) as usize;
    CheckError {
        message: d.message.clone(),
        src: src.clone(),
        span: SourceSpan::new((d.span.start as usize).into(), len),
        help: d.suggestion_source.clone(),
    }
}

/// Load a `ShEx` output descriptor from `--shape` (or an auto-discovered
/// sibling `<stem>.shex`). Returns `AcceptAll` when no schema is found, or a
/// miette error if a supplied schema fails to read/parse.
///
/// This discharges the Phase-3 deferred AcceptAll→ShEx wiring for the CLI path:
/// a real `ShEx` schema is parsed into an [`OutputDescriptorKind::ShEx`] the
/// host owns. The descriptor stays a plain value, never a Salsa key (ADR-0020).
fn load_descriptor(source: &Path, shape: Option<&Path>) -> miette::Result<OutputDescriptorKind> {
    let shape_path = shape.map(PathBuf::from).or_else(|| {
        let sibling = source.with_extension("shex");
        sibling.exists().then_some(sibling)
    });

    let Some(shape_path) = shape_path else {
        return Ok(OutputDescriptorKind::ACCEPT_ALL_DEFAULT);
    };

    let bytes = std::fs::read(&shape_path)
        .map_err(|e| miette::miette!("read shape {}: {e}", shape_path.display()))?;
    let descriptor = fossil_descriptors_output::ShExDescriptor::from_reader(bytes.as_slice())
        .map_err(|e| miette::miette!("parse ShEx shape {}: {e:?}", shape_path.display()))?;
    tracing::debug!(shape = %shape_path.display(), "loaded ShEx output descriptor");
    Ok(OutputDescriptorKind::ShEx(descriptor))
}

/// Build a fresh `FossilDb` over the CLI [`System`] for `path` + `text`.
fn open_db(text: String, path: &Path) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
    let system: Arc<dyn fossil_base::System> = Arc::new(CliSystem);
    let db = fossil_base::FossilDb::new(system);
    let file = fossil_base::SourceFile::new(&db, text, path.to_string_lossy().into_owned());
    (db, file)
}

/// `fossil check`: drain the Salsa `Diagnostic` accumulator across every
/// mapping, render each error rustc-style via miette, exit non-zero iff any
/// `Severity::Error` was accumulated (CLI-02 / SC#1).
fn cmd_check(path: &Path, shape: Option<&Path>) -> miette::Result<()> {
    tracing::debug!(?path, "fossil check");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    // Eagerly validate any supplied shape (a malformed `--shape` is itself a
    // reportable error before any per-mapping work).
    let _descriptor = load_descriptor(path, shape)?;

    let named = NamedSource::new(path.to_string_lossy(), text.clone());
    let (db, file) = open_db(text, path);

    let def_map = fossil_hir::def_map::def_map(&db, file);
    let mappings = def_map.mappings(&db);

    // Drain the accumulator per mapping: running `typecheck_mapping` pushes the
    // diagnostics, then `accumulated::<Diagnostic>` collects them.
    let mut errors: Vec<CheckError> = Vec::new();
    let mut had_error = false;
    for mapping in mappings {
        let _ = fossil_hir::check::typecheck_mapping(&db, *mapping);
        let diags = fossil_hir::check::typecheck_mapping::accumulated::<Diagnostic>(&db, *mapping);
        for d in diags {
            if d.severity == Severity::Error {
                had_error = true;
            }
            errors.push(to_check_error(d, &named));
        }
    }

    if had_error {
        // Wrap the per-mapping errors in one aggregate report so miette renders
        // them rustc-style (color + spans + help via GraphicalReportHandler);
        // returning `Err` makes `main`'s `miette::Result` exit non-zero.
        return Err(miette::Report::new(CheckReport { related: errors }));
    }

    // Non-error diagnostics (warnings/info) still render, but check succeeds.
    if !errors.is_empty() {
        for e in errors {
            eprintln!("{:?}", miette::Report::new(e));
        }
    }
    println!("ok — no errors in {}", path.display());
    Ok(())
}

/// Lower a single-mapping `.fossil` file to its [`fossil_codegen::SqlPlan`].
/// Shared by `compile` and `run`. Errors if the file declares no mapping.
fn lower_to_plan<'db>(
    db: &'db fossil_base::FossilDb,
    file: fossil_base::SourceFile,
    path: &Path,
) -> miette::Result<fossil_codegen::SqlPlan<'db>> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mappings = def_map.mappings(db);
    let mapping = mappings
        .first()
        .copied()
        .ok_or_else(|| miette::miette!("no mapping found in {}", path.display()))?;
    Ok(fossil_codegen::codegen_sql(db, mapping))
}

/// Retarget the flat-COPY `'output.parquet'` literal in the generated SQL to an
/// absolute path inside `out_dir`, so `--out-dir` lands artifacts there rather
/// than the process cwd (the COPY target is a relative literal otherwise).
/// Returns `(sql, output_parquet_path)`.
fn retarget_output(sql: &str, out_dir: &Path) -> (String, PathBuf) {
    let parquet = out_dir.join("output.parquet");
    let rewritten = sql.replace(
        "TO 'output.parquet'",
        &format!("TO '{}'", parquet.display()),
    );
    (rewritten, parquet)
}

/// `fossil compile`: write `output.parquet` + `manifest.yaml` to `--out-dir`
/// (default cwd) and print the artifact paths (CLI-01). Executes the COPY but
/// does not print a row-count summary — that is `run`'s job.
fn cmd_compile(path: &Path, out_dir: Option<&Path>, shape: Option<&Path>) -> miette::Result<()> {
    tracing::debug!(?path, ?out_dir, "fossil compile");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let _descriptor = load_descriptor(path, shape)?;

    let out_dir = out_dir.map_or_else(
        || std::env::current_dir().expect("cwd is readable"),
        Path::to_path_buf,
    );
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| miette::miette!("create out-dir {}: {e}", out_dir.display()))?;

    let (db, file) = open_db(text, path);
    let plan = lower_to_plan(&db, file, path)?;

    let manifest_path = out_dir.join("manifest.yaml");
    std::fs::write(&manifest_path, plan.manifest_yaml(&db))
        .map_err(|e| miette::miette!("write {}: {e}", manifest_path.display()))?;

    let (sql, parquet_path) = retarget_output(plan.sql(&db), &out_dir);
    fossil_runtime::execute(&sql).map_err(|e| miette::miette!("execute: {e}"))?;

    println!(
        "wrote {} and {}",
        parquet_path.display(),
        manifest_path.display()
    );
    Ok(())
}

/// `fossil run`: compile + execute via native `DuckDB`, then read the produced
/// Parquet back to print a row-count summary (CLI-03 / RESEARCH Pitfall 6).
///
/// The v0.1 flat-triple output is a single `output.parquet`; the summary counts
/// its rows (= emitted triples). Vertex/edge decomposition under a `ShEx`
/// descriptor produces per-table chunk files — those are summarised once the
/// CLI drives the descriptor path; for the `AcceptAll` case the triple count is
/// the minimal, honest summary.
fn cmd_run(path: &Path, shape: Option<&Path>) -> miette::Result<()> {
    tracing::debug!(?path, "fossil run");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let _descriptor = load_descriptor(path, shape)?;

    let out_dir = std::env::current_dir().expect("cwd is readable");
    let (db, file) = open_db(text, path);
    let plan = lower_to_plan(&db, file, path)?;

    std::fs::write(out_dir.join("manifest.yaml"), plan.manifest_yaml(&db))
        .map_err(|e| miette::miette!("write manifest.yaml: {e}"))?;

    let (sql, parquet_path) = retarget_output(plan.sql(&db), &out_dir);
    fossil_runtime::execute(&sql).map_err(|e| miette::miette!("execute: {e}"))?;

    // Read the produced Parquet back for the result summary (Pitfall 6): the
    // runtime already links DuckDB, so a follow-up count query is cheap.
    let triples = count_parquet_rows(&parquet_path)?;
    println!("ran {}: wrote {triples} triples", path.display());
    Ok(())
}

/// Count rows in a produced Parquet file via an in-memory `DuckDB` connection.
fn count_parquet_rows(parquet: &Path) -> miette::Result<i64> {
    let conn =
        duckdb::Connection::open_in_memory().map_err(|e| miette::miette!("open duckdb: {e}"))?;
    conn.query_row(
        &format!("SELECT COUNT(*) FROM read_parquet('{}')", parquet.display()),
        [],
        |row| row.get(0),
    )
    .map_err(|e| miette::miette!("count rows in {}: {e}", parquet.display()))
}
