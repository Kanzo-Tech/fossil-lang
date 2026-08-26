//! `fossil` — the native CLI binary (`run` / `check` / `providers` / `refs`).
//!
//! A thin shell over [`fossil_engine`]: it parses args, reads files/stdin, calls
//! the engine, and renders the result — rustc-style miette diagnostics for
//! `check`, machine `RunReport`/list JSON (or a human summary) for the rest. ALL
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

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use fossil_base::{Diagnostic, Severity};
use fossil_df::RunReport;
use fossil_introspect::RunCreds;
// What the corpus claims about itself, for the one line the operator reads.
use fossil_sinks::manifest::Privacy;
use miette::{GraphicalReportHandler, GraphicalTheme, NamedSource, SourceSpan};
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
        /// Emit the manifest as JSON on stdout instead of the human summary —
        /// a `RunReport`: `graph.graph.yml` plus every per-type document, the
        /// `dest` the corpus cannot know about itself, and how many rows of each
        /// edge's input resolved no endpoint. Used by keasy when invoking
        /// `fossil run` via subprocess.
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
        /// Path to the privacy policy the release is verified against — an ODRL
        /// document in fossil's privacy profile. The run REFUSES rather than
        /// writing a corpus that does not satisfy it.
        ///
        /// Omit and the corpus is sealed `privacy: undeclared`, which says on
        /// the artifact that no bound was checked. It does not say the data is
        /// public and no reader may take it that way.
        #[arg(long, value_name = "PATH")]
        policy: Option<PathBuf>,
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

/// Pin the diagnostic theme so a rendered diagnostic is the same text under a
/// terminal, under a pipe and under CI.
///
/// miette's `GraphicalTheme::default()` decides BOTH halves of the theme from
/// one question — `!stdout().is_terminal() || !stderr().is_terminal()` — and
/// answers it with `GraphicalTheme::none()`, which is ASCII box-drawing AND no
/// colour. So the frame a diagnostic draws changes with how the process was
/// invoked: `╭─[file:6:12]` interactively and `,-[file:6:12]` through a pipe,
/// from the same binary on the same input. Any golden over that text is pinned
/// to the ambient environment rather than to the compiler, and
/// `check_diagnostics.rs` was: it passed run-to-run and failed under a captured
/// harness, differing in nothing but the glyphs.
///
/// The two halves are separated here, because only one of them is ambient by
/// right. **Colour** genuinely depends on the terminal, and on `NO_COLOR` —
/// that stays. **Glyphs** do not: unicode always, so the frame is a constant.
/// `crates/fossil-engine/tests/programs.rs` reached the same conclusion for the
/// same reason and pins `unicode_nocolor()` for its committed artefacts; this
/// is that decision moved to where the CLI actually renders, so the two agree
/// by construction instead of by coincidence.
fn install_diagnostic_theme() {
    let no_color = matches!(std::env::var("NO_COLOR"), Ok(s) if s != "0");
    let colour = !no_color && std::io::stderr().is_terminal();
    let theme = if colour {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    // `set_hook` fails only if a report was already rendered or a hook already
    // installed; this runs first thing in `main`, so neither can have happened.
    let _ = miette::set_hook(Box::new(move |_| {
        Box::new(GraphicalReportHandler::new_themed(theme.clone()))
    }));
}

fn main() -> miette::Result<()> {
    install_diagnostic_theme();
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
            policy,
        } => cmd_run(
            &file,
            &dest,
            output_json,
            creds_stdin,
            memory_gib,
            policy.as_deref(),
        ),
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
/// exit non-zero iff any `Severity::Error` was accumulated.
fn cmd_check(path: &Path) -> miette::Result<()> {
    // `check` has no `--creds-stdin`, so it introspects with no connection map —
    // and against the same directory `run` will.
    introspect(path, &HashMap::new(), &RunCreds::default())?;
    let outcome = fossil_engine::check(path)?;
    let named = NamedSource::new(path.to_string_lossy(), outcome.source);

    let mut errors: Vec<CheckError> = Vec::new();
    let mut had_error = false;
    for d in &outcome.diagnostics {
        if d.severity == Severity::Error {
            had_error = true;
        }
        errors.push(to_check_error(d, &named));
        // The other half of a two-file report, right after the half it belongs
        // to — `related()` renders in order, so a reader meets the program's
        // line and then the shape's.
        errors.extend(document_errors(d, &outcome.documents));
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
    // Clean, but empty: the file parsed and declared no mapping, so it compiles
    // to no graph. Still exit 0 — nothing in it is wrong — but do not print the
    // same line a real program gets, or `check` reads as a pass on a file that
    // `fossil run` will refuse with `no mapping found`.
    if outcome.mappings == 0 {
        println!(
            "ok — no errors in {}, but it declares no mapping and would build no graph",
            path.display()
        );
        return Ok(());
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

/// `fossil run`: compile + execute, then report the resulting `RunReport`.
/// Fill the compiler's descriptor cache before asking it to compile — the one
/// pre-compile job a native host owes, and the reason `fossil-engine` links no
/// database.
///
/// `fossil-wasm` does the same from the other side: `@fossil-lang/introspect`
/// describes through the page's DuckDB-WASM and calls
/// `registerInferredDescriptor`. Same engine, same dialect, so the two hosts
/// answer one program the same way — which is what
/// `packages/introspect/tests/rust-parity.test.ts` is there to catch.
fn introspect(
    path: &Path,
    connections: &HashMap<String, String>,
    creds: &RunCreds,
) -> miette::Result<()> {
    // `host_system` takes the program PATH and derives the directory itself —
    // the cache is keyed by that directory, so a caller deriving it differently
    // would fill a table the compile never reads.
    fossil_introspect::introspect_program(
        &*fossil_engine::host_system(path),
        path,
        connections,
        creds,
    )
    .map_err(|e| miette::miette!("read {}: {e}", path.display()))
}

fn cmd_run(
    path: &Path,
    dest: &str,
    output_json: bool,
    creds_stdin: bool,
    memory_bytes: Option<u64>,
    policy_path: Option<&Path>,
) -> miette::Result<()> {
    let creds = if creds_stdin {
        RunCreds::from_stdin().map_err(|e| miette::miette!("{e}"))?
    } else {
        RunCreds::default()
    };
    // Introspect, then compile — the host's order, and the browser's. `run` used
    // to take the whole `RunCreds` and do this itself; it takes connection URLS
    // now and never sees a secret. The credential exists so that a `DESCRIBE`
    // over a cloud `@conn` source authenticates, which happens here.
    let connections = fossil_introspect::connection_urls(&creds.connections);
    introspect(path, &connections, &creds)?;
    // Read and parsed HERE, before the compile, so a malformed policy is a
    // message about the policy rather than a run that gets most of the way and
    // then cannot say what it was checking against.
    let policy = policy_path
        .map(|p| {
            let text = std::fs::read_to_string(p)
                .map_err(|e| miette::miette!("read policy `{}`: {e}", p.display()))?;
            fossil_policy::parse(&text)
                .map_err(|e| miette::miette!("policy `{}`: {e}", p.display()))
        })
        .transpose()?;
    let run = fossil_engine::run(path, dest, &connections, memory_bytes, policy.as_ref())?;
    report(&run, output_json);
    Ok(())
}

/// Emit the [`RunReport`] as machine JSON (`--output-json`, for the keasy host)
/// or a terse human summary.
///
/// **The summary names the drops, and the JSON is not the only place they
/// appear.** An endpoint that resolved no vertex is discarded on purpose, and
/// the whole point of counting it was that the discard used to be invisible; a
/// count only a `--output-json` reader sees is invisible to the operator who ran
/// the command. A run that dropped nothing says nothing extra.
fn report(run: &RunReport, output_json: bool) {
    if output_json {
        println!(
            "{}",
            serde_json::to_string(run).expect("RunReport serialises")
        );
        return;
    }
    println!(
        "wrote {} vertex type(s), {} edge type(s) to {}",
        run.vertices.len(),
        run.edges.len(),
        run.dest,
    );
    // What the corpus claims, on the line the operator reads. A run that
    // verified a bound and a run that did not are different outcomes, and
    // «undeclared» is the one worth saying out loud — it is the outcome of
    // forgetting `--policy`, and the whole reason forgetting is survivable is
    // that it is visible.
    match &run.graph.privacy {
        Privacy::Undeclared => {
            println!("  privacy: undeclared — no policy was given, so no bound was checked");
        }
        Privacy::KAnonymity(bound) => println!(
            "  privacy: k-anonymity, k={} asked and k={} reached over {} record(s){}",
            bound.k,
            bound.reached,
            bound.population,
            if bound.suppressed == 0 {
                String::new()
            } else {
                format!(", {} suppressed", bound.suppressed)
            },
        ),
    }
    for drops in run.dropped.iter().filter(|d| d.dropped > 0) {
        println!(
            "  {} — {} input row(s) named an endpoint no vertex carries, and are not edges",
            drops.prefix, drops.dropped,
        );
    }
}

/// A single type-check diagnostic, rendered rustc-style by miette's
/// `GraphicalReportHandler` (source span + caret art + color + `help:`).
///
/// `labels` is how a diagnostic about a RELATION between two places renders:
/// «`Users` and `Imported` mint two identities for Person» underlines both
/// `@subject` lines and names each. When it is empty the single `span` is
/// underlined with the generic «here», which is every other diagnostic —
/// `fossil_base::Diagnostic::labels` carries the contract.
#[derive(thiserror::Error, miette::Diagnostic, Debug)]
#[error("{message}")]
struct CheckError {
    message: String,
    #[source_code]
    src: NamedSource<String>,
    #[label(collection)]
    labels: Vec<miette::LabeledSpan>,
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
/// the diagnostic's own prose, else the structured `suggestion_source`, else an
/// inline did-you-mean clause.
fn to_check_error(d: &Diagnostic, src: &NamedSource<String>) -> CheckError {
    let help = d
        .help
        .clone()
        .or_else(|| d.suggestion_source.clone())
        .or_else(|| extract_did_you_mean(&d.message));
    CheckError {
        message: d.message.clone(),
        src: src.clone(),
        labels: labels_of(d),
        help,
    }
}

/// The spans to underline IN THE PROGRAM: the diagnostic's own labels when it
/// has them, else its single span under the generic «here».
///
/// Labels that name a document are not here — see [`document_errors`]. miette
/// resolves every range against the one `SourceCode` its report carries, so a
/// `.shex` offset rendered against the program's text would underline whatever
/// happens to sit at that byte, inside the right file and silently.
fn labels_of(d: &Diagnostic) -> Vec<miette::LabeledSpan> {
    if d.labels.is_empty() {
        return vec![at(d.span, "here")];
    }
    d.labels
        .iter()
        .filter(|l| l.document.is_none())
        .map(|l| at(l.span, &l.text))
        .collect()
}

fn at(span: fossil_base::Span, text: &str) -> miette::LabeledSpan {
    miette::LabeledSpan::new_with_span(
        Some(text.to_string()),
        SourceSpan::new(
            (span.start as usize).into(),
            span.end.saturating_sub(span.start) as usize,
        ),
    )
}

/// One extra [`CheckError`] per shape document a diagnostic points into.
///
/// A type error is about two texts — `total = Purchase.reference` in the
/// program, `shop:total xsd:float` in the shape — and one miette report has one
/// source, so the second text is a second report under the same `related()`
/// list. Its message is the same sentence: what a reader is following is one
/// complaint, and repeating it above the second snippet is what makes the two
/// legible as halves of it.
///
/// A label whose document `outcome.documents` does not carry is DROPPED. That
/// happens when the document could not be read, which is the same thing the
/// checker saw, and one missing label beats a range resolved against the wrong
/// text.
fn document_errors(d: &Diagnostic, documents: &[(String, String)]) -> Vec<CheckError> {
    let mut out: Vec<CheckError> = Vec::new();
    for label in &d.labels {
        let Some(name) = label.document.as_deref() else {
            continue;
        };
        let Some((_, text)) = documents.iter().find(|(n, _)| n == name) else {
            continue;
        };
        let span = at(label.span, &label.text);
        // Several labels in ONE document share a snippet — `colliding-name`
        // underlines two lines of the same `.shex` — so they are collected onto
        // the report that already names it rather than opening a second.
        if let Some(existing) = out.iter_mut().find(|e| e.src.name() == name) {
            existing.labels.push(span);
        } else {
            out.push(CheckError {
                message: d.message.clone(),
                src: NamedSource::new(name, text.clone()),
                labels: vec![span],
                // The `help:` belongs to the program's report; repeating it
                // under every snippet would say one repair three times.
                help: None,
            });
        }
    }
    out
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
        // for run/refs/providers), and a host parsing it as JSON must see
        // ONLY the payload. `fmt()` defaults to stdout, which corrupts that contract
        // under `RUST_LOG=debug` (keasy's `POST /v1/refs` choked on the salsa trace).
        .with_writer(std::io::stderr)
        .try_init();
}
