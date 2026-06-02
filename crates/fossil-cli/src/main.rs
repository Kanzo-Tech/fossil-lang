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

mod creds;

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
        /// Read a JSON cloud-credentials payload from stdin (see [`creds`]).
        /// Secrets must not ride argv/env on a shared host, so a multi-tenant
        /// caller pipes the dest + per-`@conn` `DuckDB` cloud-config in this way.
        /// Omit for local / public-URL runs.
        #[arg(long)]
        creds_stdin: bool,
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
            creds_stdin,
        } => cmd_run(&file, shape.as_deref(), dest.as_deref(), output_json, creds_stdin),
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
    connections: &HashMap<String, creds::ConnectionCreds>,
) {
    let conn = match duckdb::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("DuckDB in-memory open failed; skipping pre-introspection: {e}");
            return;
        }
    };
    // Cloud `@conn` sources need their read creds applied before DESCRIBE.
    // Best-effort: a creds failure just degrades this source to no forward
    // propagation (same contract as a DESCRIBE failure below).
    if let Err(e) = apply_source_creds(&conn, connections) {
        tracing::warn!("applying source creds for pre-introspection failed: {e}");
    }
    for (source_name, url) in extract_source_refs(source_text) {
        // Resolve `@conn/path` → cloud URL via the connection map; other URIs
        // pass through to the existing pass-through / relative-path handling.
        let url = resolve_source_uri(url.as_str(), connections);
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
    // No `--creds-stdin` on `check` — `@conn` cloud sources degrade to no
    // forward propagation (the empty connection map resolves nothing).
    let source_dir = path.parent().unwrap_or_else(|| Path::new("."));
    pre_introspect_and_register(db.system(), &text, source_dir, &HashMap::new());

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
    pre_introspect_and_register(db.system(), &text, source_dir, &HashMap::new());

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
    creds_stdin: bool,
) -> miette::Result<()> {
    tracing::debug!(?path, ?dest, creds_stdin, "fossil run");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let descriptor = load_descriptor(path, shape)?;

    // Cloud credentials (if any) arrive on stdin — never argv/env (secrets on a
    // shared host). Absent `--creds-stdin` ⇒ empty config ⇒ local/public-URL
    // behaviour is byte-identical to before.
    let creds = if creds_stdin {
        creds::RunCreds::from_stdin().map_err(|e| miette::miette!("{e}"))?
    } else {
        creds::RunCreds::default()
    };

    let (db, file) = open_db(text.clone(), path);
    let source_dir = path.parent().unwrap_or_else(|| Path::new("."));
    pre_introspect_and_register(db.system(), &text, source_dir, &creds.connections);

    // `--dest` present ⇒ the W0b writer path, regardless of `--shape`: the
    // output descriptor is program-resident (a `ShEx` refines it when supplied,
    // otherwise AcceptAll synthesises the vertex decomposition from the typed
    // mapping). This is the path keasy drives via subprocess — it passes a dest
    // and NO shape (the shape lives in the program). No `--dest` keeps the
    // legacy cwd flat-write walking-skeleton ("5 triples") unchanged.
    if let Some(dest_url) = dest {
        return cmd_run_w0b(&db, file, path, &descriptor, dest_url, output_json, &creds);
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
    creds: &creds::RunCreds,
) -> miette::Result<()> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mapping = def_map
        .mappings(db)
        .first()
        .copied()
        .ok_or_else(|| miette::miette!("no mapping found in {}", path.display()))?;
    let mir = fossil_mir::lower_to_mir(db, mapping);

    // Drive the W0b/5 SinkPlan bridge. The source-URI resolver maps `@conn/path`
    // bindings to their cloud URL via the stdin connection map; the view READER
    // gets the resolved URL while the view NAME stays the literal (see
    // `decompose_for_writer`).
    let chunk_size = fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;
    let resolve = |uri: &str| resolve_source_uri(uri, &creds.connections);
    let (prelude_sql, sink_plan) =
        fossil_codegen::decompose_for_writer(db, mapping, mir, descriptor, chunk_size, &resolve);

    let write_options = fossil_sinks::writer::WriteOptions::default();
    let write_plan =
        fossil_sinks::writer::plan_writes_from_sink_plan(&sink_plan, dest_url, &write_options)
            .map_err(|e| miette::miette!("plan_writes: {e}"))?;
    let manifests = fossil_sinks::writer::plan_manifests_from_sink_plan(&sink_plan, &write_options)
        .map_err(|e| miette::miette!("plan_manifests: {e}"))?;

    let conn =
        duckdb::Connection::open_in_memory().map_err(|e| miette::miette!("open duckdb: {e}"))?;
    // Apply every source connection's read cloud-config BEFORE the prelude runs
    // its read_csv_auto over cloud `@conn` sources.
    apply_source_creds(&conn, &creds.connections)?;
    conn.execute_batch(&prelude_sql)
        .map_err(|e| miette::miette!("create source views: {e}"))?;

    // Thread the dest's cloud secret (supplied on stdin) into the resolved path;
    // `materialize` installs it via a scoped `CREATE SECRET` before every COPY.
    // No secret (local/public dest) ⇒ no statement ⇒ unchanged.
    let resolved = match &creds.dest.secret {
        Some(spec) => fossil_resolver::ResolvedPath::with_secret(dest_url, spec.to_cloud_secret()),
        None => fossil_resolver::ResolvedPath::new(dest_url),
    };

    // Local dests need their directory tree pre-created — DuckDB COPY writes a
    // file but won't `mkdir -p`. Cloud object stores are flat and need none.
    if let Some(dest_dir) = local_dest_dir(dest_url) {
        std::fs::create_dir_all(&dest_dir)
            .map_err(|e| miette::miette!("create dest dir: {e}"))?;
        for s in &write_plan.vertex_statements {
            if let Some(parent) = dest_dir.join(&s.rel_path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| miette::miette!("create vertex dir: {e}"))?;
            }
        }
        for s in &write_plan.edge_statements {
            if let Some(parent) = dest_dir.join(&s.csr_rel_path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| miette::miette!("create edge dir: {e}"))?;
            }
        }
    }

    // YAML manifests go through DuckDB too, making it the SINGLE byte-writer for
    // both Parquet and YAML — the manifest write inherits the same cloud SET
    // config and needs no second storage stack / credential vocabulary. A
    // single-column row with `QUOTE ''` writes the value verbatim; DuckDB adds
    // one row-terminator `\n`, so strip the content's trailing newline for a
    // byte-exact file. (Validated against DuckDB 1.3.0, 2026-06-02.)
    let write_yaml = |rel_path: &str, content: &str| -> Result<(), String> {
        let url = resolved.join(rel_path);
        let body = content.strip_suffix('\n').unwrap_or(content);
        let sql = format!(
            "COPY (SELECT '{}' AS x) TO '{}' (FORMAT csv, HEADER false, QUOTE '', DELIMITER ',')",
            body.replace('\'', "''"),
            url.url().replace('\'', "''"),
        );
        conn.execute_batch(&sql).map_err(|e| e.to_string())
    };

    fossil_runtime::materialize_graph_ar(&conn, &write_plan, &manifests, &resolved, write_yaml)
        .map_err(|e| miette::miette!("materialize: {e}"))?;

    // W3.1b — replace the placeholder x/y/cluster_id with a real WCC partition +
    // deterministic layout, per vertex type using its self-edges. Destination-
    // agnostic: targets are URLs (`file://` or cloud), so the enrichment runs the
    // same on both. `edge_statements` and `manifests.edges` are parallel (same
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
                .map(|(es, _)| resolved.join(&es.csr_rel_path).url().to_string())
                .collect();
            fossil_runtime::layout::VertexLayoutTarget {
                vertex_parquet: resolved.join(&vstmt.rel_path).url().to_string(),
                self_edge_csr,
            }
        })
        .collect();
    fossil_runtime::layout::enrich_layout(&conn, &layout_targets)
        .map_err(|e| miette::miette!("layout: {e}"))?;

    if output_json {
        // Machine-readable status for the keasy subprocess host. Carries the
        // output graph's STRUCTURE — per vertex type: file + row count + property
        // columns; per edge type: the CSR/CSC file pair + endpoints + count — so
        // the host (which has no DuckDB of its own) need not re-introspect the
        // dataset. Column-value statistics (n_unique/min/max/samples) are
        // deliberately absent: the browser data plane computes them on demand via
        // DuckDB-WASM over the mounted Parquet. (Host-boundary: the server ships
        // structure, the browser owns stats.) `count(*)` reads the Parquet footer
        // metadata — fast, and cloud-safe (the conn still carries the SET config).
        let count_rows = |rel_path: &str| -> Option<i64> {
            let url = resolved.join(rel_path);
            conn.query_row(
                &format!(
                    "SELECT count(*) FROM read_parquet('{}')",
                    url.url().replace('\'', "''")
                ),
                [],
                |r| r.get::<_, i64>(0),
            )
            .ok()
        };

        let vertices: Vec<serde_json::Value> = write_plan
            .vertex_statements
            .iter()
            .zip(&sink_plan.vertices)
            .map(|(vstmt, vtable)| {
                let columns: Vec<serde_json::Value> = vtable
                    .properties
                    .iter()
                    .map(|p| serde_json::json!({ "name": p.name, "data_type": p.data_type }))
                    .collect();
                serde_json::json!({
                    "type": vstmt.type_name,
                    "file": vstmt.rel_path,
                    "count": count_rows(&vstmt.rel_path),
                    "columns": columns,
                })
            })
            .collect();

        let edges: Vec<serde_json::Value> = write_plan
            .edge_statements
            .iter()
            .zip(&manifests.edges)
            .map(|(estmt, em)| {
                serde_json::json!({
                    "edge_type": em.edge_info.edge_type,
                    "src_type": em.edge_info.src_type,
                    "dst_type": em.edge_info.dst_type,
                    "by_source": estmt.csr_rel_path,
                    "by_target": estmt.csc_rel_path,
                    "count": count_rows(&estmt.csr_rel_path),
                })
            })
            .collect();

        let status = serde_json::json!({
            "dest": dest_url,
            "vertices": vertices,
            "edges": edges,
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

/// Resolve a `.fossil` source URI through the `--creds-stdin` connection map.
/// `@conn/path` → `<connection url>/path`; any other URI (a direct `s3://`/
/// `https://` URL or a local path) is returned verbatim. Mirrors the keasy
/// host resolver, sourced from stdin instead of an in-process registry.
fn resolve_source_uri(raw: &str, connections: &HashMap<String, creds::ConnectionCreds>) -> String {
    let Some((conn_name, path)) = raw.strip_prefix('@').and_then(|r| r.split_once('/')) else {
        return raw.to_string(); // not an @conn reference — pass through
    };
    connections.get(conn_name).map_or_else(
        || raw.to_string(),
        |c| {
            format!(
                "{}/{}",
                c.url.trim_end_matches('/'),
                path.trim_start_matches('/')
            )
        },
    )
}

/// Install each source connection's scoped read secret on `conn` (via
/// [`fossil_runtime::install_secret`], scope = the connection URL), so a
/// `read_csv_auto` over a cloud `@conn` source authenticates. No-op for
/// connections without a secret (local / public-URL sources). Secret names are
/// per-connection-unique (scope, not name, drives DuckDB's match).
fn apply_source_creds(
    conn: &duckdb::Connection,
    connections: &HashMap<String, creds::ConnectionCreds>,
) -> miette::Result<()> {
    for (i, c) in connections.values().enumerate() {
        if let Some(spec) = &c.secret {
            let resolved = fossil_resolver::ResolvedPath::with_secret(&c.url, spec.to_cloud_secret());
            fossil_runtime::install_secret(conn, &resolved, &format!("__fossil_src_{i}"))
                .map_err(|e| miette::miette!("install source secret: {e}"))?;
        }
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

/// The local filesystem directory a dest URL writes under, or `None` when the
/// dest is a cloud object store (`s3://`, `az://`, `https://`, …) that needs no
/// directory pre-creation. `file://` URLs and bare paths
/// (`fossil run x.fossil --dest /tmp/g`) are local; the parquet/YAML COPYs
/// themselves target the URL verbatim (DuckDB dereferences `file://` and cloud
/// schemes alike), so this is used ONLY to `mkdir -p` the local tree.
fn local_dest_dir(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        return Some(PathBuf::from(rest));
    }
    if url.contains("://") {
        return None; // cloud scheme — flat namespace, no mkdir
    }
    Some(PathBuf::from(url))
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn conns(pairs: &[(&str, &str)]) -> HashMap<String, creds::ConnectionCreds> {
        pairs
            .iter()
            .map(|(name, url)| {
                (
                    (*name).to_string(),
                    creds::ConnectionCreds {
                        url: (*url).to_string(),
                        secret: None,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn resolves_at_conn_to_connection_url() {
        let c = conns(&[("sales", "s3://bucket/prefix")]);
        assert_eq!(
            resolve_source_uri("@sales/2024/orders.csv", &c),
            "s3://bucket/prefix/2024/orders.csv"
        );
    }

    #[test]
    fn collapses_slashes_at_the_join() {
        let c = conns(&[("sales", "s3://bucket/prefix/")]);
        assert_eq!(
            resolve_source_uri("@sales/x.csv", &c),
            "s3://bucket/prefix/x.csv"
        );
    }

    #[test]
    fn passes_through_direct_urls_and_paths() {
        let c = conns(&[("sales", "s3://bucket")]);
        assert_eq!(
            resolve_source_uri("s3://other/x.csv", &c),
            "s3://other/x.csv"
        );
        assert_eq!(resolve_source_uri("examples/users.csv", &c), "examples/users.csv");
    }

    #[test]
    fn unknown_connection_passes_through_verbatim() {
        // Unresolved `@conn` stays literal — the downstream read_csv_auto then
        // fails loudly rather than the CLI silently inventing a URL.
        let c = conns(&[("sales", "s3://bucket")]);
        assert_eq!(resolve_source_uri("@missing/x.csv", &c), "@missing/x.csv");
    }
}
