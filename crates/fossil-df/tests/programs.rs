//! **The conformance set, executed.** Every program under `docs/programs/`,
//! compiled by the production path and run through [`fossil_df::Executor`],
//! each one's output or diagnostic kept as an artefact the documentation
//! transcludes.
//!
//! # The artefacts
//!
//! Two files per program, under `<program>/expected/`:
//!
//! - `diagnostic.txt` — every diagnostic, rendered as a host renders it (miette
//!   graphical, unicode, no colour, width 100). Empty says the program produces
//!   no diagnostic.
//! - `compiled.txt` — what the compiler understood: type bindings and their
//!   shape IRIs, source bindings with their pipelines, per mapping the
//!   properties written against the properties lowered, then the types
//!   `fossil.graph.yml` declares and every relation's dropped count.
//!
//! `UPDATE_EXPECT=1 cargo test -p fossil-df --test programs` rewrites them
//! (`expect-test`).
//!
//! # The invariants no blessing can satisfy
//!
//! 1. **No property is lost in silence.** Every mapping's written count equals
//!    its lowered count, or an error diagnostic covers the property.
//! 2. The programs outside `errors/` produce no error; those under it produce
//!    at least one.
//! 3. Every `type { … }` binding outside `errors/` resolves to a shape IRI.
//! 4. The programs outside `errors/` run and write at least one vertex type.
//! 5. The store holds `fossil.graph.yml` and exactly the files the information
//!    files name.
//!
//! Every failure is collected and printed as one map rather than aborting at
//! the first.

/// What the compiler UNDERSTOOD, which is the `compiled.txt` half of every
/// artefact below. This file is its only caller, which is why it is a test
/// module.
mod census;
#[path = "support/native.rs"]
mod native;
mod support;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use census::ProgramCensus;
use fossil_graph_schema::{Problem, Severity};

/// One program of the set.
struct Program {
    /// `hello`, `errors/type/property-mismatch` — relative to `docs/programs/`, and what
    /// every failure line names.
    name: String,
    /// The `.fossil` file.
    source: PathBuf,
    /// Where the artefacts go.
    expected_dir: PathBuf,
    /// Under `errors/`: the checker is meant to reject it.
    must_fail: bool,
}

fn programs_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/programs")
        .canonicalize()
        .expect("docs/programs is on disk")
}

/// Every `.fossil` under the corpus, discovered rather than listed.
fn discover(root: &Path) -> Vec<Program> {
    let mut found = Vec::new();
    walk(root, &mut found);
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

fn walk(dir: &Path, out: &mut Vec<Program>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            // `data/` holds CSV and JSON; `expected/` holds what this file
            // writes; `editor/` holds a fixture for a docs widget. None holds a
            // program.
            let base = path.file_name().unwrap_or_default().to_string_lossy();
            if base == "data" || base == "expected" || base == "editor" {
                continue;
            }
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "fossil") {
            let program_dir = path.parent().expect("a program has a directory").to_owned();
            let root = programs_root();
            let name = program_dir
                .strip_prefix(&root)
                .unwrap_or(&program_dir)
                .to_string_lossy()
                .into_owned();
            out.push(Program {
                must_fail: name.starts_with("errors/") || name.starts_with("errors\\"),
                name,
                source: path,
                expected_dir: program_dir.join("expected"),
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────── rendering

/// What the run wrote, rendered without a single machine-dependent byte: the
/// types `fossil.graph.yml` declares, in its order, and the run's dropped count
/// beside each relation. The destination is not here.
///
/// The column list is the property group's, so it carries `subject` beside the
/// program's own properties, and a multi-valued one is marked `[set]` — those
/// are declared and a reader gets them. What each mapping wrote is the section
/// above this one; this section is what the graph says. The type's IRI is the
/// mapping's, and is not here.
fn render_run(corpus: &native::Corpus) -> String {
    let graph = corpus.graph();
    let mut out = String::from("\n── run ──\n");
    if graph.vertices.is_empty() {
        out.push_str("NO VERTEX TYPE was written\n");
    }
    for v in &graph.vertices {
        let columns: Vec<String> = v
            .property_groups
            .iter()
            .flat_map(|g| &g.properties)
            .map(|p| match p.cardinality {
                graphar_info::Cardinality::Single => p.name.clone(),
                _ => format!("{} [{}]", p.name, p.cardinality),
            })
            .collect();
        let _ = writeln!(
            out,
            "vertex {} × {} — {}",
            v.r#type,
            corpus.vertex_count(&v.r#type),
            columns.join(", "),
        );
    }
    if graph.edges.is_empty() {
        out.push_str("NO EDGE TYPE was written\n");
    }
    for e in &graph.edges {
        let dropped = corpus
            .report
            .dropped
            .iter()
            .find(|d| d.table == e.triple())
            .map_or(0, |d| d.dropped);
        let _ = writeln!(
            out,
            "edge {} × {}{}",
            e.triple(),
            corpus.edge_count(&e.triple()),
            // Silent on a clean run: an artefact that says «0 dropped» on every
            // program stops being read, and the one program that drops a row is
            // the whole reason the number exists.
            if dropped == 0 {
                String::new()
            } else {
                format!(", {dropped} input row(s) dropped")
            },
        );
    }
    out
}

/// Invariant 5: the store holds `fossil.graph.yml`, the mapping beside it and
/// exactly the files the information files name — each document, each chunk,
/// each count and each offset table.
fn unnamed_or_missing(corpus: &native::Corpus) -> Vec<String> {
    use graphar_parquet::paths;
    let graph = corpus.graph();
    let mut named: Vec<String> = vec!["fossil.graph.yml".to_string(), "mapping.ttl".to_string()];
    for v in &graph.vertices {
        named.push(v.file_name());
        named.push(paths::vertex_count(v));
        let count = corpus.vertex_count(&v.r#type);
        for group in &v.property_groups {
            named.extend(
                (0..count.div_ceil(v.chunk_size)).map(|k| paths::vertex_chunk(v, group, k)),
            );
        }
    }
    for e in &graph.edges {
        let adj = &e.adj_lists[0];
        named.push(e.file_name());
        named.push(paths::edge_vertex_count(e, adj));
        let sources = corpus.vertex_count(&e.src_type);
        for part in 0..sources.div_ceil(e.src_chunk_size) {
            named.push(paths::edge_count(e, adj, part));
            named.push(paths::offset_chunk(e, adj, part));
        }
        let edges = corpus.edge_count(&e.triple());
        if edges > 0 {
            named.push(paths::adj_list_chunk(e, adj, 0, 0));
            assert!(
                edges <= e.chunk_size,
                "{}: every relation is one chunk",
                e.triple()
            );
        }
    }
    let mut findings = Vec::new();
    for path in &named {
        if !corpus.files.contains_key(path) {
            findings.push(format!(
                "  RUN `{path}` is what the information files tell a reader to fetch, and it is not there"
            ));
        }
    }
    for path in corpus.files.keys() {
        if !named.contains(path) {
            findings.push(format!(
                "  RUN `{path}` was written and no information file names it"
            ));
        }
    }
    findings
}

// ───────────────────────────────────────────────────────────────── the harness

#[test]
fn the_clean_programs_compile_and_keep_what_they_say() {
    let set = discover(&programs_root());
    assert!(!set.is_empty(), "docs/programs holds no program");

    let mut report = String::new();
    let mut failed = 0usize;
    let mut artefacts: Vec<(PathBuf, String)> = Vec::new();

    for program in &set {
        let mut findings: Vec<String> = Vec::new();
        let file_name = program
            .source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        // ── stage 1: compile, through the check host ──────────────────────────
        let outcome = native::check(&program.source);

        let errors: Vec<_> = outcome
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();

        if program.must_fail && errors.is_empty() {
            findings.push(
                "  CHECK it is under errors/ and the checker accepted it — the diagnostic \
                 this program exists to produce is not produced"
                    .to_string(),
            );
        }
        if !program.must_fail && !errors.is_empty() {
            for d in &errors {
                findings.push(format!("  CHECK {}", d.message()));
            }
        }
        if !program.must_fail && outcome.mappings == 0 {
            findings.push(
                "  PARSE the file yielded no mapping at all — it parsed to nothing, or the \
                 parser recovered nothing"
                    .to_string(),
            );
        }

        // ── stage 2: the census — what survived being read ────────────────────
        let census = census::census(&program.source);

        // The measured trap, and the distinction that makes it precise.
        //
        // A property can be lost two ways and they are not the same defect.
        // `lower_binary` refuses arithmetic with a diagnostic naming the
        // expression and THEN returns `None`: the author is told, the corpus is
        // short a column, and the language owes an implementation.
        // `lower_property` returns `None` for a form it cannot read and
        // `body.rs`'s bare `if let Some(prop)` skips it with no diagnostic and
        // no counter: the author is told nothing at all. Only the second is a
        // silent drop, and only the second can pass `check` clean.
        //
        // A drop counts as ANNOUNCED when an error diagnostic's span overlaps
        // the property's. That is best-effort — a diagnostic carries one span,
        // and spans are rebased per mapping — but it is the only correlation
        // available while the lowering does not report what it discarded, and
        // getting it wrong costs a mislabel, never a missed drop.
        for (mapping, key, (start, end)) in census.drops() {
            let announced = errors
                .iter()
                .find(|d| d.span.start < end && d.span.end > start);
            match announced {
                Some(d) => findings.push(format!(
                    "  DROPPED `{mapping}.{key}` is not written to the corpus, and the \
                     compiler said so: {}",
                    d.message(),
                )),
                None => findings.push(format!(
                    "  SILENT-DROP `{mapping}` wrote `{key}` and the lowering threw it away \
                     without a word (fossil-hir/src/body.rs, the `if let Some(prop)` with no else)"
                )),
            }
        }

        // Every type binding must bind: a document that decodes to nothing
        // leaves the mapping with no output contract, and without one no
        // property is writable.
        if !program.must_fail {
            for t in &census.types {
                if t.shape_iri.is_none() {
                    findings.push(format!(
                        "  SHAPE `{}` ← {} bound no shape: {:?}",
                        t.name,
                        t.document.as_deref().unwrap_or("no document"),
                        t.error,
                    ));
                }
            }
        }

        // ── stage 3: the artefacts ────────────────────────────────────────────
        let diagnostic = native::render_diagnostics(
            &outcome.source,
            &file_name,
            &outcome.diagnostics,
            &outcome.documents,
        );
        let mut compiled = census.render();

        // ── stage 4: run, for the twenty that are meant to produce a graph ──
        //
        // Not attempted when the compile already failed: `run` would fail for the
        // same reason and the second message says nothing the first did not.
        if !program.must_fail {
            if errors.is_empty() {
                // The program's directory, served as it would be to any reader
                // of the file: every relative source and document resolves
                // beside the program, and nothing consults a working directory.
                let dir = program.source.parent().expect("a program has a directory");
                match native::run_dir(dir, &file_name, &[]) {
                    Ok(corpus) => {
                        compiled.push_str(&render_run(&corpus));
                        if corpus.graph().vertices.is_empty() {
                            findings.push(
                                "  RUN it ran and wrote no vertex type — the artefact is empty"
                                    .to_string(),
                            );
                        }
                        findings.extend(unnamed_or_missing(&corpus));
                    }
                    Err(e) => {
                        let _ = write!(compiled, "\n── run ──\nREFUSED: {e}\n");
                        findings.push(format!("  RUN {e}"));
                    }
                }
            } else {
                compiled.push_str("\n── run ──\nnot attempted: the compile failed\n");
            }
        }

        artefacts.push((program.expected_dir.join("diagnostic.txt"), diagnostic));
        artefacts.push((program.expected_dir.join("compiled.txt"), compiled));

        record(&mut report, &mut failed, &program.name, &findings);
    }

    for (path, produced) in &artefacts {
        expect_test::expect_file![path].assert_eq(produced);
    }
    assert!(
        failed == 0,
        "\n{failed} of {} conformance programs are not what they say they are.\n\n{report}",
        set.len(),
    );
}

fn record(report: &mut String, failed: &mut usize, name: &str, findings: &[String]) {
    if findings.is_empty() {
        return;
    }
    *failed += 1;
    let _ = writeln!(report, "━━ {name}");
    for f in findings {
        let _ = writeln!(report, "{f}");
    }
    report.push('\n');
}

/// The census is a compiler query and is asserted on its own, over a program
/// whose answer is known by reading it: `hello.fossil` writes `@subject` and
/// `name`.
#[test]
fn the_census_counts_hello_by_hand() {
    let hello = programs_root().join("hello/hello.fossil");

    let census: ProgramCensus = census::census(&hello);
    assert_eq!(
        census.written(),
        2,
        "hello.fossil has two PROPERTY nodes; the parser found {}",
        census.written()
    );
    assert_eq!(
        census.lowered(),
        2,
        "hello.fossil lowers to two properties; {} survived, and the ones that did not are {:?}",
        census.lowered(),
        census.drops(),
    );
}

/// A check of `text`, written as the only file of a fresh directory.
fn check_text(text: &str) -> native::CheckOutcome {
    let dir = native::write_dir(&[("subject.fossil", text)]);
    native::check(&dir.path().join("subject.fossil"))
}

/// An EMPTY file: zero mappings, zero diagnostics. Nothing in it is wrong, and
/// it builds no graph — the mapping count is what tells the two apart, because
/// «no errors» on a file that declares nothing is not the same answer as a
/// clean program.
#[test]
fn an_empty_file_has_no_diagnostic_and_no_mapping() {
    let outcome = check_text("");
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert_eq!(outcome.mappings, 0);
}

/// A WHOLLY-UNPARSEABLE file: zero mappings, and parse errors a drain over the
/// mappings alone could not reach. Before the file-level drain it read as clean.
#[test]
fn an_unparseable_file_reports_its_parse_error() {
    let outcome = check_text("!@#$%^&*() )))\n{{{ ]]]\n");
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error
                && matches!(
                    d.problem,
                    Problem::UnexpectedToken {} | Problem::UnknownCharacter { .. }
                )),
        "garbage is not a program; got {:?}",
        outcome.diagnostics
    );
}

/// The double-report guard. `def_map` sits in EVERY mapping's dependency
/// subtree, so draining it alongside the per-mapping loop would publish each
/// parse error twice. One mapping, one parse error — a property written without
/// its `=`, which the parser recovers from cleanly — one diagnostic about it.
///
/// The count is `assert_eq!`, never `>= 1`: "at least once" is the assertion
/// this test would pass with the bug it exists to catch.
#[test]
fn a_parse_error_in_a_file_with_a_mapping_is_reported_once() {
    let outcome = check_text(concat!(
        "type { Person } := io.shex(\"person.shex\")\n",
        "\n",
        "users := io.csv(\"users.csv\")\n",
        "\n",
        "User : Person from users\n",
        "    @subject = \"https://example.org/user/{users.id}\"\n",
        "    name users.name\n",
    ));
    let occurrences = outcome
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(&d.problem,
            Problem::ExpectedToken { expected, found } if expected == "ASSIGN" && found == "IDENT")
        })
        .count();
    assert_eq!(
        occurrences, 1,
        "the parse error must be reported exactly once; got {:?}",
        outcome.diagnostics
    );
}
