//! `fossil` — the native CLI binary (`run` / `check` / `catalog` / `providers`
//! / `refs`).
//!
//! A thin shell over [`fossil_engine`]: it parses args, reads files/stdin, calls
//! the engine, and renders the result — rustc-style miette diagnostics for
//! `check`, machine `RunStatus`/list JSON (or a human summary) for the rest. ALL
//! orchestration (the compile→run pipeline + the registry surface) lives in the
//! engine crate; this binary owns only the CLI + presentation.
//!
//! **WALKING-SKELETON INVARIANT:** `fossil run examples/hello.fossil --dest <tmp>`
//! must keep succeeding end-to-end, producing a valid `GraphAr` dataset (5 `Person`
//! vertices). The `tests/` integration suite drives the binary as a black box.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-cli is native-only (depends on fossil-engine which uses bundled DuckDB); \
     do not add it to the WASM CI gate"
);

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use fossil_base::{Diagnostic, Severity};
use fossil_engine::{CatalogRequest, RunCreds};
use fossil_run_status::RunStatus;
use miette::{NamedSource, SourceSpan};
use tracing_subscriber::EnvFilter;

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
    /// Parse + type-check a `.fossil` file, rendering rustc-style miette
    /// diagnostics. Exits 0 on a clean file; non-zero if any error diagnostic
    /// was accumulated.
    Check {
        /// Path to the `.fossil` source file.
        file: PathBuf,
    },
    /// Compile + execute a `.fossil` file via native `DuckDB`, materialising
    /// `GraphAr` Parquet under `--dest` (the W0b column shape) plus YAML
    /// manifests. The output descriptor is program-resident — the single source
    /// of truth (`io.rdf(schema = …)` `ShEx`, else synthesised) — never a flag.
    Run {
        /// Path to the `.fossil` source file.
        file: PathBuf,
        /// Destination URL for `GraphAr` output (`file:///path`,
        /// `s3://bucket/prefix`, …). Required.
        #[arg(long)]
        dest: String,
        /// Emit a machine-readable `RunStatus` JSON on stdout instead of the
        /// human summary. Used by keasy when invoking `fossil run` via subprocess.
        #[arg(long)]
        output_json: bool,
        /// Read a JSON cloud-credentials payload from stdin. Secrets must not ride
        /// argv/env on a shared host. Omit for local / public-URL runs.
        #[arg(long)]
        creds_stdin: bool,
        /// Memory budget for the run, in gibibytes (`--memory-gib 4`, fractions
        /// allowed). One number for both engines: the executor spills to disk
        /// instead of growing past it, and the layout pass runs under the same
        /// limit. Omit for unbounded — a corpus larger than the machine then
        /// dies rather than slows down.
        #[arg(long, value_name = "GIB", value_parser = gib_to_bytes)]
        memory_gib: Option<u64>,
    },
    /// Materialise a DCAT-AP catalog graph (`GraphAr`) from a `CatalogInput`
    /// piped on stdin. The host supplies governance values + the run's dataset
    /// structure; fossil owns the DCAT-AP shape and writes it via the `run` writer.
    Catalog {
        /// Destination URL for the `GraphAr` catalog output. The dest cloud secret
        /// (if any) rides the stdin payload.
        #[arg(long)]
        dest: String,
        /// Emit the `RunStatus` JSON on stdout (consumed by the keasy host).
        #[arg(long)]
        output_json: bool,
    },
    /// List the data-source providers fossil supports (the `io.*` source
    /// constructors). The host reads this to populate its connector UI.
    Providers {
        /// Emit the `Vec<ProviderInfo>` JSON on stdout (consumed by keasy).
        #[arg(long)]
        output_json: bool,
    },
    /// List a program's external references — every `@conn`/URL/path a source
    /// constructor names (data, `schema =`). The TYPED lineage keasy reads to
    /// know a job's connections. Parse-only: no `DuckDB`, no credentials.
    Refs {
        /// Path to the `.fossil` source file.
        file: PathBuf,
        /// Emit the `Vec<SourceRefInfo>` JSON on stdout (consumed by keasy).
        #[arg(long)]
        output_json: bool,
    },
}

fn main() -> miette::Result<()> {
    miette::set_panic_hook();
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Commands::Check { file } => cmd_check(&file),
        Commands::Run {
            file,
            dest,
            output_json,
            creds_stdin,
            memory_gib,
        } => cmd_run(&file, &dest, output_json, creds_stdin, memory_gib),
        Commands::Catalog { dest, output_json } => {
            let req = CatalogRequest::from_stdin().map_err(|e| miette::miette!(e))?;
            cmd_catalog(&dest, output_json, &req)
        }
        Commands::Providers { output_json } => cmd_providers(output_json),
        Commands::Refs { file, output_json } => cmd_refs(&file, output_json),
    }
}

/// `fossil refs`: parse-only lineage, JSON or `role\tconn\tpath` lines.
fn cmd_refs(path: &Path, output_json: bool) -> miette::Result<()> {
    let refs = fossil_engine::refs(path)?;
    if output_json {
        println!(
            "{}",
            serde_json::to_string(&refs).expect("Vec<SourceRefInfo> serialises")
        );
    } else {
        for r in &refs {
            println!(
                "{:?}\t{}\t{}",
                r.role,
                r.connection.as_deref().unwrap_or("-"),
                r.path
            );
        }
    }
    Ok(())
}

/// `fossil providers`: the `io.*` data-source providers fossil owns.
fn cmd_providers(output_json: bool) -> miette::Result<()> {
    let providers = fossil_engine::providers();
    if output_json {
        let json = serde_json::to_string(&providers).map_err(|e| miette::miette!(e))?;
        println!("{json}");
    } else {
        for p in &providers {
            println!("{} ({})", p.name, p.extensions.join(", "));
        }
    }
    Ok(())
}

/// `fossil check`: render each accumulated diagnostic rustc-style via miette;
/// exit non-zero iff any `Severity::Error` was accumulated (CLI-02 / SC#1).
fn cmd_check(path: &Path) -> miette::Result<()> {
    let outcome = fossil_engine::check(path)?;
    let named = NamedSource::new(path.to_string_lossy(), outcome.source);

    let mut errors: Vec<CheckError> = Vec::new();
    let mut had_error = false;
    for d in &outcome.diagnostics {
        if d.severity == Severity::Error {
            had_error = true;
        }
        errors.push(to_check_error(d, &named));
    }

    if had_error {
        // One aggregate report so miette renders them rustc-style; `Err` makes
        // `main`'s `miette::Result` exit non-zero.
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

/// `--memory-gib` → bytes, the form both engines are configured in. Rejects
/// anything that is not a positive size: a budget of zero (or of NaN) is not a
/// tighter run, it is a run in which the first allocation fails.
fn gib_to_bytes(raw: &str) -> Result<u64, String> {
    let gib: f64 = raw
        .parse()
        .map_err(|_| format!("`{raw}` is not a number of gibibytes"))?;
    if !gib.is_finite() || gib <= 0.0 {
        return Err(format!("memory budget must be positive, got `{raw}`"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok((gib * 1024.0 * 1024.0 * 1024.0) as u64)
}

/// `fossil run`: compile + execute, then report the resulting `RunStatus`.
fn cmd_run(
    path: &Path,
    dest: &str,
    output_json: bool,
    creds_stdin: bool,
    memory_bytes: Option<u64>,
) -> miette::Result<()> {
    let creds = if creds_stdin {
        RunCreds::from_stdin().map_err(|e| miette::miette!("{e}"))?
    } else {
        RunCreds::default()
    };
    let status = fossil_engine::run(path, dest, &creds, memory_bytes)?;
    report(&status, output_json);
    Ok(())
}

/// `fossil catalog`: materialise the DCAT-AP graph, then report.
fn cmd_catalog(dest: &str, output_json: bool, req: &CatalogRequest) -> miette::Result<()> {
    let status = fossil_engine::catalog(dest, req)?;
    report(&status, output_json);
    Ok(())
}

/// Emit a `RunStatus` as machine JSON (`--output-json`, for the keasy host) or a
/// terse human summary.
fn report(status: &RunStatus, output_json: bool) {
    if output_json {
        println!(
            "{}",
            serde_json::to_string(status).expect("RunStatus serialises")
        );
    } else {
        println!(
            "wrote {} vertex type(s), {} edge type(s) to {}",
            status.vertices.len(),
            status.edges.len(),
            status.dest,
        );
    }
}

/// A single type-check diagnostic, rendered rustc-style by miette's
/// `GraphicalReportHandler` (source span + caret art + color + `help:`).
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
/// rustc-style report (the `#[related]` list).
#[derive(thiserror::Error, miette::Diagnostic, Debug)]
#[error("type-checking failed: {} error(s)", related.len())]
struct CheckReport {
    #[related]
    related: Vec<CheckError>,
}

/// Convert a drained [`Diagnostic`] into a [`CheckError`]. The `#[help]` line is
/// the structured `suggestion_source`, else an inline did-you-mean clause.
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

/// Initialise `tracing-subscriber` with `EnvFilter`. `RUST_LOG` overrides the
/// default; `--verbose` upgrades the implicit default from `warn` to `debug`.
fn init_tracing(verbose: bool) {
    let default_directive = if verbose { "fossil=debug" } else { "warn" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_directive));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        // Logs go to STDERR: stdout is the machine-readable channel (`--output-json`
        // for run/refs/providers/catalog), and a host parsing it as JSON must see
        // ONLY the payload. `fmt()` defaults to stdout, which corrupts that contract
        // under `RUST_LOG=debug` (keasy's `POST /v1/refs` choked on the salsa trace).
        .with_writer(std::io::stderr)
        .try_init();
}
