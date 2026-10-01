//! **The conformance set, executed.** `docs/programs/` — twenty-one clean
//! programs,
//! compiled by the production path, each one's output or diagnostic kept as an
//! artefact.
//!
//! `grammar.bnf`'s header names this directory as the language's only mechanical
//! control: *«THE CONFORMANCE SET is `docs/programs/` … Nothing mechanically
//! checks this file against the parser today, so those 26 programs are the only
//! check it has.»* Until this file existed that sentence was false in the one way
//! that matters — **nobody compiled them.** They were transcluded by the
//! documentation, and the only thing checked was that the file and its
//! `// #region` were on disk (`docs/lib/programs.ts`). A stale example rotted
//! in silence instead of failing to compile, and the five `expected/diagnostic.txt`
//! were hand-written prose that no compiler produced and nothing compared.
//!
//! # Why here and not in the web app
//!
//! The thing that has to go red is the COMPILER. A script under `docs/` that
//! compiled the programs would put the failure on the documentation build,
//! which is the wrong side of the seam: a parser regression is not a docs
//! problem, and the docs build already has two ways to be red for reasons that
//! are its own.
//!
//! # Why `fossil-df`
//!
//! Because [`fossil_df::Executor`] is the one host that writes a corpus, and a
//! program is kept here for the corpus it writes. This file lived beside the
//! native `fossil` binary, whose `fossil run` wrote a different corpus from the
//! same program; that host was deleted on 2026-09-30, and what it did around
//! the compiler is `tests/support/native.rs` now — a check host (the real
//! filesystem, the whole provider registry, every source described first, the
//! documents a program names registered from disk) and a run through the
//! executor over the program's directory served from memory. So a `.shex`
//! sitting on disk beside the program is still enough, and neither half
//! registers a document the program did not name.
//!
//! # `tests/write.rs` is next door and is not this
//!
//! `the_corpus_keeps_the_promises_it_makes_to_a_stranger` writes ONE generated
//! graph and validates the `fossil/1` corpus it produces with numbered SQL checks
//! over `DuckDB` — `dense_id` global and gapless, the edge endpoints, the row
//! groups, the Hilbert order. It is one corpus in great depth. This file is
//! every program at the depth of «did it compile, did it keep every property the
//! author wrote, and is every table `fossil.json` names in the store». Both are
//! needed and neither substitutes for the other.
//!
//! # What replaces the `file:line` citation check
//!
//! `docs/content.test.ts` scans every MDX page for `` `path/to/file.rs:12` ``
//! and asserts the line exists — **that the line exists, not that it says what
//! the page claims.** This file is the replacement for the claims that are
//! PROGRAMS. It is not a replacement for the ones that are not.
//!
//! # The artefacts
//!
//! Two files per program, under `<program>/expected/`:
//!
//! - `diagnostic.txt` — every diagnostic, rendered exactly as `fossil check`
//!   rendered it (miette graphical, unicode, no colour, width 100). Empty is a
//!   statement, not an absence: it says this program produces no diagnostic, and
//!   it goes red the day that stops being true.
//! - `compiled.txt` — what the compiler UNDERSTOOD. Type bindings and the shape
//!   IRI each resolved to, source bindings, and per mapping the properties
//!   written against the properties lowered. Then the tables `fossil.json`
//!   declares — each with its `record_count` and its columns — and every
//!   relation's dropped count from the run's report, for every program that is
//!   not under `errors/`.
//!
//!   A source binding that DERIVES a relation carries its pipeline, rendered from
//!   the HIR by `fossil_hir::display` — the join key, the filter predicate, the
//!   self-join's alias. It used to carry `LineRow.join(?)`, and the counts alone
//!   cannot stand in for it: drop the `tenant` conjunct from `compound-key` and
//!   the join goes from four rows to seven, and the seven mint the same four
//!   subjects. Every number in the artefact is equal across a change that breaks
//!   the program. The predicate is the only thing that is not.
//!
//! `FOSSIL_BLESS=1 cargo test -p fossil-df --test programs` regenerates them.
//! A diagnostic text nobody produces is a promise the compiler does not make.
//!
//! # Every `expected/diagnostic.txt` is blessed, and five were hand-written
//!
//! They described the diagnostics the language was MEANT to produce, and
//! blessing one before the compiler could produce it would have overwritten an
//! accurate description of the target with an accurate description of today —
//! the difference between them being the specification. **The routine, when the
//! next one is written: bless a program the day it says what it means, and
//! `git checkout --` the rest**, because `FOSSIL_BLESS=1` writes all of them.
//!
//! What the five needed, in the order they were closed:
//!
//! - **`two-identities` needed nothing.** Message, both spans, both labels and
//!   the `help` were already identical; what differed was a `[{severity:?}]`
//!   prefix this file's own renderer added and `fossil check` did not (see
//!   `native::render_diagnostics`), and the context lines around the snippet. Both
//!   are how the report is DRAWN, and the hand-written file was drawn by hand.
//! - **`unknown-field` needed a second label in the same file and a narrower
//!   caret.** The label is the line that bound the row, saying which fields it
//!   has; the caret is on `nmae` rather than on `User.nmae`, which took a span
//!   per REFERENCE where the compiler recorded one per property
//!   (`fossil_hir::body::HirBody::ref_spans`).
//! - **`wrong-type`, `colliding-name` and `missing-property` needed the report
//!   to cite TWO FILES** — a label under a line of the `.shex`, which
//!   [`fossil_base::SpanLabel`] could not express because it carried a span and
//!   a frame and no file. It carries a `document` now; the range comes from
//!   `fossil_shex::spans`, a lookup over the document's text; and a renderer
//!   groups labels by file, one `Rendered` per file under one message. The LSP
//!   publishes per URI and so is part of the same capability — `fossil-lsp` and
//!   `fossil-wasm` do not route these yet, which is the next thing.
//!
//! # Three differences from the hand-written targets are DELIBERATE
//!
//! - **The document's vocabulary is not the program's.** The targets wrote
//!   `shop:Order declares shop:total as xsd:float` and
//!   `` `shop:phone` is optional ``; the compiler writes `` `Order` declares
//!   `total` as Float `` and `` `phone` is optional ``. Only a resolved IRI
//!   reaches `fossil-hir` — the CURIE is the document's prefix map, which stops
//!   at the decoder — and the bare name is the word the author typed on the
//!   line above anyway.
//! - **A binding label underlines the whole binding**, not just the
//!   constructor call: `type { Person } := io.shex("shape.shex")` and
//!   `User := io.csv("data/users.csv")` entire, because the label says what
//!   `Person`/`User` IS and the name is part of that sentence.
//! - **Block layout is miette's**, including which snippet comes first.
//!
//! # One thing the blessing deleted, and it came back
//!
//! `wrong-type`'s target carried a `help:` the compiler did not produce —
//! *«`Purchase.amount` is Float. If `reference` really holds the number,
//! `parse.float(Purchase.reference)` converts it.»* — so for one commit this
//! header was the only description of it. Both halves exist now
//! (`fossil_hir::check::repair_for`): a column of the EXPECTED type on the same
//! row, and a stdlib row taking the actual type and returning the expected one.
//! The second is a SEARCH over the catalogue rather than a table of pairs,
//! because the catalogue is data.
//!
//! The wording differs by one word — «really holds the value» where the target
//! wrote «really holds the number», because the sentence is generated for every
//! pair of types and only one of them is a number.
//!
//! # Three causes of failure, and only one of them is the parser
//!
//! A red run is a map, and it is only a useful map if the reader can tell the
//! three apart. Every finding below is prefixed with its stage for that reason:
//!
//! - `PARSE` / `SILENT-DROP` — the parser has not landed the production, or it
//!   landed and the lowering threw away what it produced.
//! - `RUN` — the program compiled and the run refused it.
//! - `SHAPE` — a type binding's document decoded to nothing, so the mapping got
//!   no output contract and no property in it is writable.
//!
//! A fourth cause is not visible from here and is named so nobody hunts for it:
//! nothing in the language produces a per-row `Iri`. `check.rs` types an
//! interpolation as `IriTemplate` only in subject position, and the RDF term
//! constructors were deleted from the catalogue — so an edge whose shape declares
//! a shape-valued range reads as `expected Iri, got String`.
//!
//! # Five of the twenty-six exist because the grammar promised and nobody paid
//!
//! `grammar.bnf`'s header claims that no production exists below it for a form
//! none of the conformance programs spells, except where a comment says so and
//! says why. The claim failed five times — `TernaryExpr` (L1), `OrExpr` (L2),
//! `AdditiveExpr` (L5), `MulExpr` (L6) and `UnaryExpr` (L7, with `not` reserved
//! and unused). It was closed with five programs rather than five comments,
//! because a comment excuses the gap and a program CLOSES it: nothing knew
//! whether those five productions worked.
//!
//! - `ternary` — `c ? a : b`, and the chained form, which is the L1
//!   right-associativity claim. It is also the only program where the `:`
//!   disambiguation of rule 3 is exercised inside a mapping body.
//! - `disjunction` — `or` in a `where` predicate and `a or b and c` in a
//!   property, which is the L2-looser-than-L3 claim.
//! - `arithmetic` — `+` and `-`, and `gross - discount + shipping`, which is a
//!   real left-associativity test: right-associating it changes the number.
//! - `scaling` — `*`, `/`, `%`, and `handling + unit_price * quantity`, which
//!   is the L5/L6 precedence claim.
//! - `negation` — unary `-` and `not`, in a predicate and in a property.
//!
//! Each exercises its production rather than mentioning it, and each is expected
//! to COMPILE: `grammar.bnf` is normative and the parser implements it, so a
//! production the grammar declares is a promise the compiler owes. Three of the
//! five are red today for reasons the report separates.
//!
//! # Blessing cannot make this green, and that is the design
//!
//! A snapshot harness that only diffs snapshots passes the moment you bless it,
//! which would be worthless here — the parser is mid-migration and the goldens
//! are meant to move. So the invariants below are checked in BOTH modes and are
//! independent of every golden:
//!
//! 1. The set is twenty-six, twenty-one clean and five under `errors/`.
//! 2. **No property is lost in silence.** The measured trap:
//!    `fossil_hir::body::body` keeps the properties `lower_property` returns
//!    `Some` for and skips the `None`s with a bare `if let` — no diagnostic, no
//!    accumulator. `name = User.name` already parses and is already thrown away,
//!    so a naive harness goes green having lost the program. Every mapping's
//!    written count must equal its lowered count.
//! 3. Non-vacuity for (2): the parser's `PROPERTY` count is checked against a
//!    SECOND, independent reader — a text scan of the program for indented
//!    `name = …` lines. If the parser stops producing `PROPERTY` nodes, (2)
//!    becomes vacuously true and only this catches it.
//! 4. The twenty-one produce no error diagnostic; the five produce at least one.
//! 5. Every `type { … }` binding in the twenty-one resolves to a shape IRI. A
//!    binding that resolved to nothing gives the mapping no output contract, and
//!    a program with no contract can write no property at all.
//! 6. The twenty-one `run` and produce at least one vertex type — the artefact
//!    each one is kept for.
//! 7. Every table `fossil.json` names is in the store when `execute` returns,
//!    and nothing else is: a reader fetches the manifest and follows it, and
//!    nothing is discovered by listing, so every path it names owes its
//!    existence.
//!
//! # It is red today, and the report is the point
//!
//! Every failure is collected and printed as one map — which program, which
//! stage, what the compiler said — rather than aborting at the first. That list
//! is the readable statement of what the surface migration has left to do, and it
//! is worth more than the assertion that produced it.

#![cfg(not(target_arch = "wasm32"))]

/// What the compiler UNDERSTOOD, which is the `compiled.txt` half of every
/// artefact below. It was `fossil-engine`'s `src/census.rs` until 2026-08-26;
/// this file is the only caller it has ever had, and its own docblock had
/// already named this as where it goes.
mod census;
#[path = "support/native.rs"]
mod native;
mod support;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use census::ProgramCensus;
use fossil_base::Severity;

/// How many programs the conformance set has, and how many of them are meant to
/// be rejected. Pinned rather than counted, because the set moving is a decision
/// and `grammar.bnf`'s header states these numbers: a program added or deleted
/// without that header changing is a divergence between the language's spec and
/// its only control.
const EXPECTED_TOTAL: usize = 30;
const EXPECTED_FAILING: usize = 6;

/// One program of the set.
struct Program {
    /// `hello`, `errors/wrong-type` — relative to `docs/programs/`, and what
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
///
/// Listing them would mean a nineteenth program could be added and never
/// compiled, which is the exact failure this file exists to end. The count is
/// asserted instead, so a program appearing or disappearing is loud.
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

// ─────────────────────────────────────────────────────── the independent reader

/// Count the properties a program writes, WITHOUT the parser.
///
/// This is the non-vacuity guard for the silent-drop invariant. If the parser
/// stops emitting `PROPERTY` nodes — which is exactly what a half-landed surface
/// change does — then written and lowered are both zero and «nothing was
/// dropped» is true and meaningless. So the count is taken a second time by a
/// reader that shares no code with the compiler: an indented line whose head is
/// an identifier (or `@subject`) followed by a single `=`.
///
/// It is deliberately crude and deliberately not a parser. `:=` does not match
/// (a `:` intervenes), `==` does not match (the second `=`), a comment does not
/// match, and a top-level binding does not match because it is not indented.
fn properties_in_text(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            if trimmed.len() == line.len() || trimmed.starts_with("//") {
                return false; // not indented, or a comment
            }
            let mut chars = trimmed.char_indices();
            let mut end = if let Some((_, '@')) = chars.clone().next() {
                chars.next();
                1
            } else {
                0
            };
            let mut saw_ident = false;
            for (i, c) in chars {
                if c.is_alphanumeric() || c == '_' {
                    saw_ident = true;
                    end = i + c.len_utf8();
                } else {
                    break;
                }
            }
            if !saw_ident {
                return false;
            }
            let rest = trimmed[end..].trim_start();
            rest.starts_with('=') && !rest.starts_with("==")
        })
        .count()
}

// ─────────────────────────────────────────────────────────────────── rendering

/// What the run wrote, rendered without a single machine-dependent byte: the
/// tables `fossil.json` declares, in its order, and the run's dropped count
/// beside each relation. The destination is not here.
///
/// The column list is the manifest's, so it carries `dense_id`, `subject` and
/// the three layout columns beside the program's own properties — those are
/// declared and a reader gets them. What each mapping wrote is the section above
/// this one; this section is what the corpus says.
fn render_run(corpus: &native::Corpus) -> String {
    let manifest = corpus.manifest();
    let mut out = String::from("\n── run ──\n");
    if manifest.vertex_tables.is_empty() {
        out.push_str("NO VERTEX TYPE was written\n");
    }
    for v in &manifest.vertex_tables {
        let columns: Vec<&str> = v.properties.iter().map(|p| p.name.as_str()).collect();
        let _ = writeln!(
            out,
            "vertex {} ({}) × {} — {}",
            v.name,
            v.iri.as_deref().unwrap_or("no rdf:type"),
            v.record_count,
            columns.join(", "),
        );
    }
    if manifest.edge_tables.is_empty() {
        out.push_str("NO EDGE TYPE was written\n");
    }
    for e in &manifest.edge_tables {
        let dropped = corpus
            .report
            .dropped
            .iter()
            .find(|d| d.table == e.name)
            .map_or(0, |d| d.dropped);
        let _ = writeln!(
            out,
            "edge {} × {}{}",
            e.name,
            e.record_count,
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

/// Invariant 7: the store holds `fossil.json` and exactly the tables it names.
fn unnamed_or_missing(corpus: &native::Corpus) -> Vec<String> {
    let manifest = corpus.manifest();
    let mut named: Vec<String> = manifest
        .vertex_tables
        .iter()
        .map(|v| v.path.clone())
        .chain(manifest.edge_tables.iter().map(|e| e.path.clone()))
        .collect();
    named.push("fossil.json".to_string());
    let mut findings = Vec::new();
    for path in &named {
        if !corpus.files.contains_key(path) {
            findings.push(format!(
                "  RUN `{path}` is what fossil.json tells a reader to fetch, and it is not there"
            ));
        }
    }
    for path in corpus.files.keys() {
        if !named.contains(path) {
            findings.push(format!(
                "  RUN `{path}` was written and fossil.json names it nowhere"
            ));
        }
    }
    findings
}

// ────────────────────────────────────────────────────────────── the artefacts

/// Compare an artefact, or write it when blessing. Returns the failure line.
///
/// The diff is reported as the two texts in full and not as a line diff: these
/// are small files, and a harness that reports «line 7 differs» about a
/// diagnostic makes the reader open two files to learn what the compiler said.
fn artefact(path: &Path, produced: &str, bless: bool) -> Option<String> {
    if bless {
        std::fs::create_dir_all(path.parent().expect("artefact has a directory"))
            .unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
        std::fs::write(path, produced).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        return None;
    }
    let on_disk = std::fs::read_to_string(path).ok();
    match on_disk {
        Some(text) if text == produced => None,
        Some(text) => Some(format!(
            "  ARTEFACT {} differs.\n  ── on disk ──\n{}\n  ── produced ──\n{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            indent(&text),
            indent(produced),
        )),
        None => Some(format!(
            "  ARTEFACT {} is not on disk. Bless with FOSSIL_BLESS=1.\n  ── produced ──\n{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            indent(produced),
        )),
    }
}

fn indent(text: &str) -> String {
    if text.is_empty() {
        return "  (empty)".to_string();
    }
    text.lines()
        .map(|l| format!("  │ {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ───────────────────────────────────────────────────────────────── the harness

#[test]
fn the_clean_programs_compile_and_keep_what_they_say() {
    let bless = std::env::var_os("FOSSIL_BLESS").is_some();
    let root = programs_root();
    let set = discover(&root);

    // The set itself, before anything is compiled. A conformance suite that
    // silently shrank would pass every check it still ran.
    let failing = set.iter().filter(|p| p.must_fail).count();
    assert_eq!(
        set.len(),
        EXPECTED_TOTAL,
        "the conformance set is {} programs, and grammar.bnf's header says {EXPECTED_TOTAL}: {:?}",
        set.len(),
        set.iter().map(|p| &p.name).collect::<Vec<_>>(),
    );
    assert_eq!(
        failing, EXPECTED_FAILING,
        "{failing} programs sit under errors/, and grammar.bnf's header says {EXPECTED_FAILING}",
    );

    let mut report = String::new();
    let mut failed = 0usize;

    for program in &set {
        let mut findings: Vec<String> = Vec::new();
        let text = std::fs::read_to_string(&program.source)
            .unwrap_or_else(|e| panic!("read {}: {e}", program.source.display()));
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
                findings.push(format!("  CHECK {}", d.message));
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
                    d.message,
                )),
                None => findings.push(format!(
                    "  SILENT-DROP `{mapping}` wrote `{key}` and the lowering threw it away \
                     without a word (fossil-hir/src/body.rs, the `if let Some(prop)` with no else)"
                )),
            }
        }

        // Non-vacuity for the line above.
        let by_text = properties_in_text(&text);
        if census.written() != by_text {
            findings.push(format!(
                "  PARSE the parser found {} PROPERTY node(s); the text has {by_text} property \
                 line(s). The silent-drop check is measured against the parser, so it is only \
                 evidence while these two agree",
                census.written(),
            ));
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
                        if corpus.manifest().vertex_tables.is_empty() {
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

        if let Some(f) = artefact(
            &program.expected_dir.join("diagnostic.txt"),
            &diagnostic,
            bless,
        ) {
            findings.push(f);
        }
        if let Some(f) = artefact(&program.expected_dir.join("compiled.txt"), &compiled, bless) {
            findings.push(f);
        }

        record(&mut report, &mut failed, &program.name, &findings);
    }

    assert!(
        failed == 0,
        "\n{failed} of {EXPECTED_TOTAL} conformance programs are not what they say they are.\n\
         This is the map of what the surface migration has left to do.\n\n{report}"
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
/// whose answer is known by reading it.
///
/// Without this the whole harness rests on `written()` and `lowered()` being
/// right, and both are computed by the thing under test. `hello.fossil` is two
/// properties by inspection — `@subject` and `name` — and the independent text
/// reader has to say two as well.
#[test]
fn the_census_counts_hello_by_hand() {
    let hello = programs_root().join("hello/hello.fossil");
    let text = std::fs::read_to_string(&hello).expect("hello.fossil");
    assert_eq!(
        properties_in_text(&text),
        2,
        "hello.fossil writes `@subject` and `name`, and the independent reader must see both"
    );

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
                && d.message.to_lowercase().contains("unexpected")),
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
        .filter(|d| d.message.contains("expected ASSIGN, found IDENT"))
        .count();
    assert_eq!(
        occurrences, 1,
        "the parse error must be reported exactly once; got {:?}",
        outcome.diagnostics
    );
}
