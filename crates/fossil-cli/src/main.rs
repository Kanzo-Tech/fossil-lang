//! `fossil` — Phase 1 walking-skeleton CLI binary.
//!
//! Three subcommands wire the full Phase 1 pipeline end-to-end:
//!
//! - `fossil compile <FILE>` → `parse` → `def_map` → `lower_to_hir` →
//!   `lower_to_mir` → `codegen_sql` → execute via `DuckDB`; writes
//!   `output.parquet` and `manifest.yaml` to the current working directory.
//! - `fossil check <FILE>`   → parse + `def_map` only; Phase 1 typecheck is
//!   trivially `Ok(())` so this exits 0 with `ok` (Phase 3 CORE-04..07 plumbs
//!   `ErrorGuaranteed` through `typecheck_mapping`; Phase 6 CLI-02 wraps the
//!   resulting diagnostics in fancy miette output).
//! - `fossil run <FILE>`     → alias for `compile` in Phase 1 (Phase 6 CLI-03
//!   may diverge once a REPL exists).
//!
//! **WALKING-SKELETON IGNITION:** at this commit the canonical demo
//! `fossil compile examples/hello.fossil` succeeds end-to-end through all 13
//! compiler+runtime crates. Subsequent commits MUST NOT regress this — see
//! ROADMAP.md sequencing rule #6 + CLAUDE.md "Hard Rules".

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-cli is native-only (depends on fossil-runtime which uses bundled DuckDB); \
     do not add it to the WASM CI gate"
);

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
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
    /// Compile a `.fossil` file end-to-end: writes `output.parquet` and
    /// `manifest.yaml` to the current working directory.
    Compile {
        /// Path to the `.fossil` source file.
        file: PathBuf,
    },
    /// Parse + type-check a `.fossil` file. Exits 0 on success. Phase 1
    /// type-check is trivial; Phase 3 wires real diagnostics.
    Check {
        /// Path to the `.fossil` source file.
        file: PathBuf,
    },
    /// Compile + execute a `.fossil` file. Phase 1: alias for `compile`.
    Run {
        /// Path to the `.fossil` source file.
        file: PathBuf,
    },
}

fn main() -> miette::Result<()> {
    miette::set_panic_hook();
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Commands::Check { file } => cmd_check(&file),
        // Phase 1: `run` is a deliberate alias for `compile`. Phase 6 CLI-03
        // may diverge (e.g. REPL); keeping the arms collapsed here is
        // intentional and removes the clippy::match_same_arms noise.
        Commands::Compile { file } | Commands::Run { file } => cmd_compile(&file),
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

/// Run the full Phase 1 compile pipeline on `path` and write the produced
/// `output.parquet` + `manifest.yaml` artefacts to the current working
/// directory.
fn cmd_compile(path: &Path) -> miette::Result<()> {
    tracing::debug!(?path, "fossil compile");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
    let db = fossil_base::FossilDb::new(system);
    let file = fossil_base::SourceFile::new(&db, text, path.to_string_lossy().into_owned());

    let def_map = fossil_hir::def_map::def_map(&db, file);
    let mappings = def_map.mappings(&db);
    let mapping = mappings
        .first()
        .copied()
        .ok_or_else(|| miette::miette!("no mapping found in {}", path.display()))?;

    let plan = fossil_codegen::codegen_sql(&db, mapping);

    // Write the GraphAr manifest to the cwd. Phase 6 CLI-02 may add an
    // `--out-dir` flag; Phase 1 deliberately keeps the surface minimal.
    std::fs::write("manifest.yaml", plan.manifest_yaml(&db))
        .map_err(|e| miette::miette!("write manifest.yaml: {e}"))?;

    // Hand the SQL batch to the native DuckDB runtime. The `COPY (…) TO
    // 'output.parquet' …` literal in the generated SQL writes Parquet to the
    // process cwd unless an absolute path is embedded.
    fossil_runtime::execute(plan.sql(&db)).map_err(|e| miette::miette!("execute: {e}"))?;

    println!("wrote output.parquet, manifest.yaml");
    Ok(())
}

/// Phase 1 trivial check: parse + build the `DefMap`. Exits 0 with `ok` if
/// the parse succeeds; Phase 3 (CORE-04..07) plumbs real `ErrorGuaranteed`
/// propagation and Phase 6 (CLI-02) wraps the resulting diagnostics in fancy
/// miette output.
fn cmd_check(path: &Path) -> miette::Result<()> {
    tracing::debug!(?path, "fossil check");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
    let db = fossil_base::FossilDb::new(system);
    let file = fossil_base::SourceFile::new(&db, text, path.to_string_lossy().into_owned());

    let _def_map = fossil_hir::def_map::def_map(&db, file);
    // Phase 1: typecheck is a trivial Ok(()) per fossil-hir's check.rs.
    println!("ok");
    Ok(())
}
