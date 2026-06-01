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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use clap::{Parser, Subcommand};
use fossil_base::{Db, Diagnostic, FsError, Severity, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_descriptors_output::{OutputDescriptorKind, SystemWithDescriptors};
use miette::{NamedSource, SourceSpan};
use smol_str::SmolStr;
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
struct CliSystem {
    /// Phase 13 INPUT-01 (ADR-0037 / plan 13-04a) — host-registered
    /// `InferredDescriptors`, keyed by source binding name (e.g. `"users"` for
    /// `users := io.csv(...)`). Populated by [`pre_introspect_and_register`]
    /// ahead of every typecheck/compile invocation.
    ///
    /// Mirrors `NativeSystem` in `fossil-base` (13-02). `Mutex` is appropriate
    /// because writes happen once per compile (pre-introspection), reads are
    /// per-mapping during typecheck — `RwLock` contention isn't justified.
    inferred: Mutex<HashMap<SmolStr, InferredDescriptor>>,
}

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

    fn inferred_descriptor(&self, source_name: &str) -> Option<InferredDescriptor> {
        self.inferred.lock().ok()?.get(source_name).cloned()
    }

    fn register_inferred_descriptor(&self, descriptor: InferredDescriptor) {
        if let Ok(mut lock) = self.inferred.lock() {
            lock.insert(descriptor.source_name.clone(), descriptor);
        }
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
    /// Compile + execute a `.fossil` file via native `DuckDB`.
    ///
    /// Two modes:
    ///   - Legacy (no `--dest`, no `--shape`): writes a single
    ///     `output.parquet` + `manifest.yaml` to cwd and prints a triple-count
    ///     summary. Walking-skeleton invariant; bit-for-bit unchanged.
    ///   - W0b (`--dest <url>` + `--shape <file>`): runs the new vertex/edge
    ///     decomposition writer, materialising `GraphAr` Parquet with the W0b
    ///     column shape (`dense_id`, `subject`, `x`, `y`, `cluster_id` on
    ///     vertices; CSR + CSC on edges) under `<url>`. With `--output-json`
    ///     emits a status JSON on stdout (consumed by keasy via subprocess).
    Run {
        /// Path to the `.fossil` source file.
        file: PathBuf,
        /// Path to a `ShEx` schema used as the output target shape (see
        /// `compile --shape`).
        #[arg(long)]
        shape: Option<PathBuf>,
        /// W0b: destination URL for `GraphAr` output (`file:///path`,
        /// `s3://bucket/prefix`, …). Triggers the W0b writer path when set
        /// AND a `--shape` is provided. Without it, the legacy cwd flat-write
        /// path runs.
        #[arg(long)]
        dest: Option<String>,
        /// W0b: emit a machine-readable status JSON on stdout instead of the
        /// human triple-count summary. Used by keasy when invoking
        /// `fossil run` via subprocess.
        #[arg(long)]
        output_json: bool,
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
        Commands::Run {
            file,
            shape,
            dest,
            output_json,
        } => cmd_run(&file, shape.as_deref(), dest.as_deref(), output_json),
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

/// Convert a drained [`Diagnostic`] into a [`CheckError`]. The `#[help]` line is
/// the structured `suggestion_source` when present; otherwise, if the message
/// carries an inline did-you-mean clause, surface it as a standalone rustc-style
/// help line. Spans are byte offsets, directly usable as a [`SourceSpan`].
///
/// `message` is the diagnostic text verbatim from the checker, which routes
/// every rendered type through `fossil_hir::render_ty_kind` — so a
/// `TyKind::Unknown` never reaches this layer (it renders as `?`).
fn to_check_error(d: &Diagnostic, src: &NamedSource<String>) -> CheckError {
    let len = d.span.end.saturating_sub(d.span.start) as usize;
    let help = d
        .suggestion_source
        .clone()
        .or_else(|| extract_did_you_mean(&d.message));
    CheckError {
        message: d.message.clone(),
        src: src.clone(),
        span: SourceSpan::new((d.span.start as usize).into(), len),
        help,
    }
}

/// If `message` contains an inline `did you mean …?` clause, lift it to a
/// standalone `help:` line so miette renders it rustc-style under the snippet.
fn extract_did_you_mean(message: &str) -> Option<String> {
    let idx = message.find("did you mean")?;
    Some(message[idx..].to_string())
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
    let system: Arc<dyn fossil_base::System> = Arc::new(CliSystem::default());
    let db = fossil_base::FossilDb::new(system);
    let file = fossil_base::SourceFile::new(&db, text, path.to_string_lossy().into_owned());
    (db, file)
}

// ----- Phase 13 INPUT-01 (ADR-0037 / plan 13-04a) — pre-introspection -----

/// Map a `DuckDB` column-type string to the canonical Fossil Primitive name
/// (matches `fossil-hir::infer::primitive_from_name` exactly).
///
/// The match table mirrors the TS sibling
/// `packages/playground/src/hooks/useInferredDescriptors.ts` (kept in sync —
/// both consumers feed the same fossil-hir primitive table).
fn duckdb_type_to_fossil_primitive(t: &str) -> &'static str {
    let upper = t.trim().to_ascii_uppercase();
    match upper.as_str() {
        "INTEGER" | "BIGINT" | "INT" | "SMALLINT" | "TINYINT" | "HUGEINT" => "Integer",
        "DOUBLE" | "FLOAT" | "REAL" => "Float",
        t if t.starts_with("DECIMAL") => "Float",
        "BOOLEAN" | "BOOL" => "Bool",
        "DATE" => "Date",
        "TIMESTAMP" | "DATETIME" => "DateTime",
        "TIME" => "Time",
        // VARCHAR / TEXT / STRING + any unrecognised type fall back to String
        // (matching the fossil-hir::infer::primitive_from_name wildcard arm).
        _ => "String",
    }
}

/// Scrape source-binding RHS source URLs from a `.fossil` file's text.
///
/// Regex-based (v0.2 placeholder). Shape mirrors the TS `extractSourceRefs`
/// in `packages/playground/src/hooks/useInferredDescriptors.ts` so playground
/// + CLI behave identically.
///
/// LIMITATIONS (documented; Phase 14+ replaces with AST walk):
/// - does NOT match multi-line constructor (`name :=\n  io.csv("...")`)
/// - does NOT match interleaved comments between `:=` and `io.csv(`
/// - does NOT handle backslash-escaped quotes inside the URL string
fn extract_source_refs(text: &str) -> Vec<(SmolStr, String)> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(\w[\w\d_]*)\s*:=\s*io\.(?:csv|json)\(\s*['"]([^'"]+)['"]"#)
            .expect("static regex")
    });
    re.captures_iter(text)
        .map(|c| {
            (
                SmolStr::from(c.get(1).unwrap().as_str()),
                c.get(2).unwrap().as_str().to_string(),
            )
        })
        .collect()
}

/// Pre-introspect every source binding and register an [`InferredDescriptor`]
/// on the [`System`] BEFORE typecheck. Failures per-source are non-fatal —
/// they log via `tracing::warn` + skip; the compile may still succeed via the
/// legacy CSVW path or with no forward propagation for that source.
fn pre_introspect_and_register(
    system: &dyn fossil_base::System,
    source_text: &str,
    source_dir: &Path,
) {
    let conn = match duckdb::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("DuckDB in-memory open failed; skipping pre-introspection: {e}");
            return;
        }
    };
    for (source_name, url) in extract_source_refs(source_text) {
        // Resolve URL:
        // - URL with http(s):// / s3:// / absolute path → pass through (DuckDB
        //   read_csv_auto handles network IO via the httpfs extension per
        //   CLAUDE.md stack pin + ADR-0037 Consequences §"Layer separation
        //   scope" — native fossil-cli's wider IO surface is explicit).
        // - Relative path → try source_dir-relative first (the v0.2 convention),
        //   fall back to verbatim (= cwd-relative) so v0.1 .fossil files (e.g.
        //   examples/hello.fossil using "examples/users.csv" relative to
        //   repo-root) keep working unchanged.
        let url_str = url.as_str();
        let is_pass_through = url_str.starts_with("http://")
            || url_str.starts_with("https://")
            || url_str.starts_with("s3://")
            || Path::new(url_str).is_absolute();
        let resolved_path = if is_pass_through {
            url_str.to_string()
        } else {
            let joined = source_dir.join(url_str);
            if joined.exists() {
                joined.to_string_lossy().into_owned()
            } else {
                url_str.to_string()
            }
        };

        // Escape single-quotes for the SQL string literal (read_csv_auto takes
        // a SQL string, not a prepared-statement parameter).
        let escaped_path = resolved_path.replace('\'', "''");
        let sql = format!("DESCRIBE SELECT * FROM read_csv_auto('{escaped_path}')");
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "DESCRIBE prepare failed for source `{source_name}` (url=`{url}`): {e}"
                );
                continue;
            }
        };
        let cols: Vec<InferredColumn> = match stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let typ: String = row.get(1)?;
            Ok(InferredColumn {
                name: SmolStr::from(name),
                primitive: SmolStr::from(duckdb_type_to_fossil_primitive(&typ)),
            })
        }) {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!("DESCRIBE query_map failed for `{source_name}`: {e}");
                continue;
            }
        };
        let descriptor = InferredDescriptor {
            source_name: source_name.clone(),
            columns: cols,
            content_hash: String::new(),
        };
        system.register_inferred_descriptor(descriptor);
        tracing::debug!("pre-registered InferredDescriptor for `{source_name}`");
    }
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
    let (db, file) = open_db(text.clone(), path);

    // Phase 13 v0.2 (ADR-0037): same pre-introspection as `cmd_compile` so
    // `fossil check` sees the same forward-propagated types the compiler will.
    let source_dir = path.parent().unwrap_or_else(|| Path::new("."));
    pre_introspect_and_register(db.system(), &text, source_dir);

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

    let (db, file) = open_db(text.clone(), path);

    // Phase 13 v0.2 (ADR-0037 / plan 13-04a): pre-introspect every io.csv /
    // io.json source binding and register the resulting InferredDescriptor on
    // the System BEFORE typecheck/codegen. Mirrors the browser-side flow
    // orchestrated by plan 13-04b.
    let source_dir = path.parent().unwrap_or_else(|| Path::new("."));
    pre_introspect_and_register(db.system(), &text, source_dir);

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

/// `fossil run`: compile + execute via native `DuckDB`.
///
/// Two modes — see the `Commands::Run` doc on the clap struct:
/// - Legacy (no `--dest`): writes `output.parquet` + `manifest.yaml` to cwd
///   and prints a triple-count summary. Walking-skeleton invariant.
/// - W0b (`--dest <url>` + `--shape <file>`): runs the W0b writer +
///   materializer (the path keasy invokes via subprocess). The vertex
///   Parquets carry the W0b column shape (`dense_id`, `subject`, `x`, `y`,
///   `cluster_id`); edges write both CSR + CSC.
fn cmd_run(
    path: &Path,
    shape: Option<&Path>,
    dest: Option<&str>,
    output_json: bool,
) -> miette::Result<()> {
    tracing::debug!(?path, ?dest, "fossil run");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let descriptor = load_descriptor(path, shape)?;

    let (db, file) = open_db(text.clone(), path);
    let source_dir = path.parent().unwrap_or_else(|| Path::new("."));
    pre_introspect_and_register(db.system(), &text, source_dir);

    // Dispatch to the W0b path when `--dest` is set AND a ShEx descriptor
    // is present. Without `--shape` the decomposition is AcceptAll (a
    // single flat-triple passthrough vertex) — the W0b writer would still
    // emit something, but the legacy cwd flat-write is a cleaner default
    // for the no-shape walking-skeleton case, so we keep it.
    if let Some(dest_url) = dest
        && shape.is_some()
    {
        return cmd_run_w0b(&db, file, path, &descriptor, dest_url, output_json);
    }

    cmd_run_legacy(&db, file, path)
}

/// W0b path — decompose, materialise to `dest_url` via the W0b writer.
fn cmd_run_w0b(
    db: &fossil_base::FossilDb,
    file: fossil_base::SourceFile,
    path: &Path,
    descriptor: &fossil_descriptors_output::OutputDescriptorKind,
    dest_url: &str,
    output_json: bool,
) -> miette::Result<()> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mapping = def_map
        .mappings(db)
        .first()
        .copied()
        .ok_or_else(|| miette::miette!("no mapping found in {}", path.display()))?;
    let mir = fossil_mir::lower_to_mir(db, mapping);

    // Drive the W0b/5 SinkPlan bridge.
    let chunk_size = fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;
    let (prelude_sql, sink_plan) =
        fossil_codegen::decompose_for_writer(db, mapping, mir, descriptor, chunk_size);

    let write_options = fossil_sinks::writer::WriteOptions::default();
    let write_plan =
        fossil_sinks::writer::plan_writes_from_sink_plan(&sink_plan, dest_url, &write_options)
            .map_err(|e| miette::miette!("plan_writes: {e}"))?;
    let manifests = fossil_sinks::writer::plan_manifests_from_sink_plan(&sink_plan, &write_options)
        .map_err(|e| miette::miette!("plan_manifests: {e}"))?;

    let conn =
        duckdb::Connection::open_in_memory().map_err(|e| miette::miette!("open duckdb: {e}"))?;
    conn.execute_batch(&prelude_sql)
        .map_err(|e| miette::miette!("create source views: {e}"))?;

    let dest_local = local_path_from_url(dest_url)?;
    let resolved = fossil_resolver::ResolvedPath::new(dest_url);

    // Local-fs YAML writer. Cloud destinations need an uploader — W0b/7.
    let write_yaml = |rel_path: &str, content: &str| -> Result<(), String> {
        let full = dest_local.join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&full, content).map_err(|e| e.to_string())
    };

    // Pre-create the vertex + edge directories so DuckDB COPY doesn't
    // need its own mkdir (DuckDB writes the file but won't create
    // intermediate dirs). Same contract as the W0b/4 integration test.
    for s in &write_plan.vertex_statements {
        if let Some(parent) = dest_local.join(&s.rel_path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| miette::miette!("create vertex dir: {e}"))?;
        }
    }
    for s in &write_plan.edge_statements {
        if let Some(parent) = dest_local.join(&s.csr_rel_path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| miette::miette!("create edge dir: {e}"))?;
        }
    }

    fossil_runtime::materialize_graph_ar(&conn, &write_plan, &manifests, &resolved, write_yaml)
        .map_err(|e| miette::miette!("materialize: {e}"))?;

    // W3.1b — replace the placeholder x/y/cluster_id with a real WCC partition +
    // deterministic layout, per vertex type using its self-edges. Local-fs dest
    // only (the COPY rewrite + rename need a real path; cloud is W0b/7).
    // `edge_statements` and `manifests.edges` are parallel (both from the same
    // SinkPlan edge order), so zipping correlates each edge to its src/dst type.
    let layout_targets: Vec<fossil_runtime::layout::VertexLayoutTarget> = write_plan
        .vertex_statements
        .iter()
        .map(|vstmt| {
            let vtype = vstmt.type_name.as_str();
            let self_edge_csr = write_plan
                .edge_statements
                .iter()
                .zip(&manifests.edges)
                .filter(|(_, em)| em.edge_info.src_type == vtype && em.edge_info.dst_type == vtype)
                .map(|(es, _)| dest_local.join(&es.csr_rel_path))
                .collect();
            fossil_runtime::layout::VertexLayoutTarget {
                vertex_parquet: dest_local.join(&vstmt.rel_path),
                self_edge_csr,
            }
        })
        .collect();
    fossil_runtime::layout::enrich_layout(&conn, &layout_targets)
        .map_err(|e| miette::miette!("layout: {e}"))?;

    if output_json {
        // Machine-readable status — keasy parses this when invoking the CLI
        // as a subprocess. Kept deliberately minimal: types/edges names +
        // their rel_paths under `dest_url`. Row counts can be derived by
        // the caller via DuckDB on the resulting files; the CLI doesn't
        // count to keep the run fast.
        let vertex_paths: Vec<&str> = write_plan
            .vertex_statements
            .iter()
            .map(|s| s.rel_path.as_str())
            .collect();
        let edge_paths: Vec<(&str, &str)> = write_plan
            .edge_statements
            .iter()
            .map(|s| (s.csr_rel_path.as_str(), s.csc_rel_path.as_str()))
            .collect();
        let status = serde_json::json!({
            "dest": dest_url,
            "vertices": vertex_paths,
            "edges": edge_paths,
        });
        println!("{status}");
    } else {
        println!(
            "ran {}: wrote {} vertex type(s), {} edge type(s) to {}",
            path.display(),
            write_plan.vertex_statements.len(),
            write_plan.edge_statements.len(),
            dest_url,
        );
    }
    Ok(())
}

/// Legacy path — preserves walking-skeleton invariant byte-for-byte.
fn cmd_run_legacy(
    db: &fossil_base::FossilDb,
    file: fossil_base::SourceFile,
    path: &Path,
) -> miette::Result<()> {
    let out_dir = std::env::current_dir().expect("cwd is readable");
    let plan = lower_to_plan(db, file, path)?;
    std::fs::write(out_dir.join("manifest.yaml"), plan.manifest_yaml(db))
        .map_err(|e| miette::miette!("write manifest.yaml: {e}"))?;
    let (sql, parquet_path) = retarget_output(plan.sql(db), &out_dir);
    fossil_runtime::execute(&sql).map_err(|e| miette::miette!("execute: {e}"))?;
    let triples = count_parquet_rows(&parquet_path)?;
    println!("ran {}: wrote {triples} triples", path.display());
    Ok(())
}

/// Translate a `file://` URL to a local filesystem path. Other URL schemes
/// (s3://, az://, https://) are rejected — cloud destinations require an
/// uploader the CLI does not yet ship (W0b/7).
fn local_path_from_url(url: &str) -> miette::Result<PathBuf> {
    url.strip_prefix("file://").map_or_else(
        || {
            if url.contains("://") {
                Err(miette::miette!(
                    "destination URL `{url}`: only `file://` paths supported in the W0b CLI; \
                     cloud destinations require the uploader landing in W0b/7"
                ))
            } else {
                // Bare paths treated as local for ergonomics
                // (`fossil run x.fossil --dest /tmp/g`).
                Ok(PathBuf::from(url))
            }
        },
        |rest| Ok(PathBuf::from(rest)),
    )
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
