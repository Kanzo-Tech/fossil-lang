//! Diagnostic golden-file corpus.
//!
//! Locks the checker's diagnostics as regression tests, and grows toward a
//! negative-test suite for SILENT SEMANTIC ERRORS: a mapping that compiles and
//! emits wrong RDF, which is the failure class this whole corpus exists to make
//! impossible.
//!
//! # Fixture layout
//!
//! Each Db-wired fixture lives under
//! `tests/fixtures/diagnostics/<bucket>/<name>/` with:
//!   - `mapping.fossil` — required (the source).
//!   - `person.shex` — the shape document the program's
//!     `type { … } := io.shex("…")` line names, in the line format
//!     `fossil_base::test_support` decodes.
//!
//! The source row is not a file on disk here: it arrives the way a host
//! supplies one, through `register_inferred`, called by the driver before the
//! check.
//!
//! # Driving strategy — "Db-wired" vs "helper-proven"
//!
//! The production `typecheck_mapping` Salsa query resolves the target shape
//! through the file registry and the source row through the source registry,
//! so forward propagation (SC#1) is exercised **end-to-end through the
//! production query path**: `run_db_wired_fixture` writes a real `SourceFile`
//! whose path lets the relative `io.shex("…")` argument resolve, then reads
//! the diagnostics the query and what it read returned. These fixtures are labelled
//! **Db-wired**.
//!
//! Backward checking (SC#2) and the value-disjunction rejection (SC#4) are NOT
//! driven end-to-end here: they drive the **plain-Rust logic the production
//! checker uses** — `ResolvedShape::from_shape` and `render_split_suggestion` —
//! over a shape document **built by hand in the neutral vocabulary**. These are
//! labelled **helper-proven**. The `Checker` struct has `pub(crate)` fields and
//! is not constructible from an integration test, so we exercise its inputs and
//! the lowering it consumes, which is exactly the logic SC#2/SC#4 assert.
//!
//! # Two locks the harness has to open, or a shape document is invisible
//!
//! Each one alone is enough to make a `.shex` written next to a fixture
//! invisible:
//!
//! 1. A `NativeSystem` whose `System::providers` is the trait default — the
//!    four rows that read DATA and none that reads types — answers `None` from
//!    `shape_document` for every document.
//! 2. `fossil_hir::shapes::decoded_document` resolves through
//!    `fossil_base::file_at`, which reads the SALSA INPUT REGISTRY and never
//!    the disk — so a document nobody called `register_file` for is not there.
//!
//! Both failures render as "this mapping resolved no shape", which is
//! indistinguishable from a fixture that named no document: a fixture would go
//! green while checking nothing. So the harness uses `fossil_base::test_support`
//! — a decoder for a line format with no schema language behind it, plus the
//! host that installs it and the registration step — and `run_db_wired_fixture`
//! registers every `.shex` sitting in the fixture directory under the key the
//! PROGRAM's relative path resolves to. Every Db-wired fixture names one.
//!
//! The fixtures' documents are that line format, not `.shex` files parsed by
//! `ShExDescriptor`: a hand-built document says what it is testing on the line
//! where it is tested, instead of in JSON three directories away, and a test of
//! the checker is not a test of the `ShEx` decoder.
//!
//! Every snapshot header records which mode the fixture used so the
//! test-helper-vs-Db-wired status is auditable in the committed golden files in `tests/snapshots/`.

// `doc_markdown`: the doc comments name bare SC identifiers (SC#1, AcceptAll,
// Db) that read naturally without backticks in this test harness.
// `literal_string_with_formatting_args`: the Fossil identity template
// `"https://example.org/u/{users.id}"` is LITERAL Fossil source passed to the
// suggestion renderer, not a Rust format string — and a `{…}` hole is a
// string clippy reads as one (the workspace allows this elsewhere — see
// check.rs).
#![allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]

use std::path::Path;
use std::path::PathBuf;

use expect_test::expect_file;
use fossil_base::register_document;
use fossil_base::test_support::new_db;
use fossil_base::{Db, Diagnostic, SourceFile};
use fossil_graph_schema::{Occurs, Primitive, PropertyConstraint, Rejection, Shape, Term};
use fossil_hir::body::body;
use fossil_hir::def_map::def_map;
use fossil_hir::shapes::ResolvedShape;
use fossil_hir::{render_split_suggestion, render_ty_kind, typecheck_mapping};

// ---------------------------------------------------------------------------
// Shared fixture infrastructure
// ---------------------------------------------------------------------------

fn fixture_dir(bucket: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/diagnostics")
        .join(bucket)
        .join(name)
}

fn read_fixture(dir: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(file)).ok()
}

/// Register every shape document in `dir` as a Salsa input, under the key a
/// program in that same directory resolves its relative path to.
///
/// This is the half that is easy to forget and impossible to notice: the
/// checker finds a document through `fossil_base::file_at`, which reads the
/// input registry. A `.shex` on disk that nobody registered is, to every query
/// in the compiler, a document that does not exist — and the message for that
/// is the same one a program with no `type { … } := io.shex(…)` line gets.
fn register_shape_documents(db: &mut dyn Db, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "shex")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            register_document(db, &path.to_string_lossy(), &text);
        }
    }
}

/// Strip the (machine-specific) absolute manifest path from a message so
/// snapshots are stable across machines / CI. Any absolute fixture path renders
/// as `<FIXTURE>/...`.
fn redact_paths(msg: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    msg.replace(manifest, "<FIXTURE>")
}

/// Render a single [`Diagnostic`] to deterministic plain text. The
/// `suggestion_source` (if present) is emitted as an indented multi-line block,
/// preserved verbatim per Minor #6 (NOT truncated at the first newline).
///
/// # The `help:` and the labels are rendered, because they are not in the message
///
/// A did-you-mean and a diagnostic's other places are fields, not clauses of
/// the sentence, so a renderer that takes only the message and the span drops
/// them — and this file's own `run_db_wired_fixture` says what that costs:
/// «the suggestions would vanish and the snapshots would record the absence as
/// the new truth». `did_you_mean_short_name_one_char` and
/// `did_you_mean_unrelated_no_suggestion` are a PAIR — one suggests and one
/// declines to — and without the `help:` line the two snapshots are identical.
fn render_diagnostic(out: &mut String, diag: &Diagnostic) {
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "{:?}: {}",
        diag.severity,
        redact_paths(&diag.message())
    );
    let _ = writeln!(out, "  at {}..{}", diag.span.start, diag.span.end);
    for label in &diag.labels {
        let _ = writeln!(
            out,
            "  label {}..{} ({:?}): {}",
            label.span.start,
            label.span.end,
            label.frame,
            redact_paths(&label.text)
        );
    }
    if let Some(help) = &diag.help {
        let _ = writeln!(out, "  help: {}", redact_paths(help));
    }
    // The structured pair `fossil_ide::code_action` turns into a one-edit
    // `WorkspaceEdit`. Recorded separately from the `help:` because they can
    // disagree and the disagreement is the bug: the prose can name a
    // replacement while `wrong_span` covers `User.nmae`, and applying THAT
    // quick-fix deletes the qualifier.
    if let Some(dym) = &diag.did_you_mean {
        let _ = writeln!(
            out,
            "  quick-fix: replace {}..{} with `{}`",
            dym.wrong_span.start, dym.wrong_span.end, dym.replacement
        );
    }
    if let Some(sugg) = &diag.suggestion_source {
        let _ = writeln!(out, "  suggestion:");
        for line in sugg.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
}

// ---------------------------------------------------------------------------
// Forward-propagation driver (Db-wired — production query path)
// ---------------------------------------------------------------------------

/// What the check of `mapping` says, and what every query it reads says: the
/// mapping's body, the file's parse and its lowering — the whole of what a
/// reader of this one mapping is told.
fn read_by_check(db: &dyn Db, mapping: fossil_hir::def_map::MappingLoc<'_>) -> Vec<Diagnostic> {
    let file = mapping.file(db);
    let mut out = typecheck_mapping(db, mapping).diagnostics(db).clone();
    out.extend(body(db, mapping).diagnostics(db).iter().cloned());
    out.extend(
        fossil_syntax::parse(db, file)
            .diagnostics(db)
            .iter()
            .cloned(),
    );
    out.extend(
        fossil_hir::lower::lower_to_hir(db, file)
            .diagnostics(db)
            .iter()
            .cloned(),
    );
    out
}

/// Drive a fixture **end-to-end through the production `typecheck_mapping`
/// query**. The `mapping.fossil`'s `io.shex("…")` path is resolved relative to
/// the fixture directory, so the target shape is bound by the real
/// `resolve_target_shape` path and the source row by the real
/// `resolve_source_scope` one.
///
/// Returns the rendered diagnostics (the golden output the golden file captures).
fn run_db_wired_fixture(bucket: &str, name: &str) -> String {
    let dir = fixture_dir(bucket, name);
    let src = read_fixture(&dir, "mapping.fossil")
        .unwrap_or_else(|| panic!("fixture {bucket}/{name} must have a mapping.fossil"));

    let mut db = new_db();
    // The SourceFile path makes a relative document path resolve against the
    // fixture directory (resolve_relative joins on the parent dir), which is the
    // join `type { … } := io.shex("…")` resolves through.
    let file_path = dir.join("mapping.fossil");
    let file = SourceFile::new(&db, src, file_path.to_string_lossy().to_string());
    register_shape_documents(&mut db, &dir);
    // **The HOST's job, and the reason these fixtures have a typed row at all.**
    // The row is the only thing that makes a `did you mean` possible: without
    // this loop the suggestions would vanish and the snapshots would record the
    // absence as the new truth.
    //
    // Introspection cannot run here — `fossil-hir` is WASM-clean and has no
    // `DuckDB`, and these directories have no CSV to introspect anyway. So the
    // host registers what an introspection WOULD have found, which is exactly
    // what a host's `program.describe` does before a compile.
    let uris: Vec<String> = def_map(&db, file)
        .sources(&db)
        .iter()
        .filter_map(|entry| entry.uri.as_deref().map(str::to_string))
        .collect();
    for uri in uris {
        {
            fossil_base::test_support::register_inferred(
                &mut db,
                &uri,
                &[
                    ("id", Primitive::Integer),
                    ("name", Primitive::STRING),
                    ("age", Primitive::Integer),
                ],
            );
        }
    }

    let mut out = String::new();
    out.push_str("# mode: Db-wired (production typecheck_mapping query)\n");
    out.push_str("# bucket: ");
    out.push_str(bucket);
    out.push('\n');

    let mappings = def_map(&db, file).mappings(&db).clone();
    let mut all_diags: Vec<Diagnostic> = Vec::new();
    for mloc in &mappings {
        all_diags.extend(read_by_check(&db, *mloc));
    }

    if all_diags.is_empty() {
        out.push_str("(no diagnostics emitted)\n");
    } else {
        // Stable ordering: by span then message.
        all_diags.sort_by(|a, b| {
            (a.span.start, a.span.end, a.message().as_str()).cmp(&(
                b.span.start,
                b.span.end,
                b.message().as_str(),
            ))
        });
        for diag in &all_diags {
            render_diagnostic(&mut out, diag);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Backward-check + disjunction driver (helper-proven — the neutral vocabulary)
// ---------------------------------------------------------------------------

/// One predicate constrained to an XSD datatype, or to nothing.
fn constraint(predicate: &str, datatype: Option<Primitive>, occurs: Occurs) -> PropertyConstraint {
    PropertyConstraint {
        name: fossil_graph_schema::local_name(predicate).to_string(),
        predicate: predicate.to_string(),
        term: datatype.map(Term::of),
        targets: Vec::new(),
        occurs,
        // A test fixture, not a document: no text to point into.
        span: None,
        annotations: Vec::new(),
        languages: Vec::new(),
    }
}

fn shape(iri: &str, properties: Vec<PropertyConstraint>) -> Shape {
    Shape {
        iri: iri.to_string(),
        properties,
        span: None,
        annotations: Vec::new(),
    }
}

/// Drive a shape document through the plain-Rust logic the production checker
/// consumes (`ResolvedShape::from_shape` → `render_split_suggestion`). Renders
/// the resolved constraint table + whatever the decoder rejected + the
/// generated split suggestion, so the golden file locks the SC#2/SC#4 surface.
fn run_shape_fixture(bucket: &str, document: &Shape, rejections: &[Rejection]) -> String {
    use std::fmt::Write as _;

    let db = new_db();
    let mut out = String::new();
    out.push_str("# mode: helper-proven (the neutral shape vocabulary, built by hand, through\n");
    out.push_str("#       ResolvedShape — these fixtures name no document, so the production\n");
    out.push_str("#       path resolves no shape; see module docs)\n");
    out.push_str("# bucket: ");
    out.push_str(bucket);
    out.push('\n');

    // Resolved constraint table (the SC#2 backward-check input surface).
    let resolved = ResolvedShape::from_shape(
        &db,
        document,
        rejections.to_vec(),
        smol_str::SmolStr::from("person.shex"),
        None,
    );
    if resolved.constraints.is_empty() {
        let _ = writeln!(
            out,
            "resolved constraints: (none — a disjunction body, see the rejection below)"
        );
    } else {
        let _ = writeln!(out, "resolved constraints:");
        for c in &resolved.constraints {
            // `(any)` is the document declining to narrow the value, and the
            // checker reads it that way: no expected type, cardinality still
            // enforced.
            let ty_text = c
                .value_ty
                .map_or_else(|| "(any)".to_string(), |t| render_ty_kind(&db, t.kind(&db)));
            let _ = writeln!(out, "  {} : {} [{:?}]", c.predicate, ty_text, c.occurs);
        }
    }

    if resolved.rejections.is_empty() {
        let _ = writeln!(out, "rejections: (none)");
    }
    for rejection in &resolved.rejections {
        let Rejection::Disjunction {
            shape_iri,
            disjuncts,
        } = rejection
        else {
            let _ = writeln!(out, "rejection: {rejection:?}");
            continue;
        };
        let _ = writeln!(
            out,
            "disjunction rejection: {} branches in shape `{shape_iri}`",
            disjuncts.len()
        );
        let _ = writeln!(
            out,
            "  branch predicates: {}",
            disjuncts
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        // The arguments are production's, in production's order: the MAPPING
        // name, the shape, the SOURCE BINDING the mapping reads, the subject
        // template. The third one is not the first — passing the mapping name
        // for both would emit a `from` clause naming the mapping being split.
        // This corpus does not go through the production call;
        // `check_tests::a_disjunction_rejection_attaches_to_the_consuming_mapping`
        // does, and pins the distinction.
        //
        // A mapping header names a bound SHAPE, not a CURIE, and an identity is
        // a quoted string with `{expr}` holes, so the header is the bare
        // `Contact` a `type { Contact } := …` binding introduces.
        let suggestion = render_split_suggestion(
            "Contact",
            "Contact",
            "users",
            "\"https://example.org/u/{users.id}\"",
            disjuncts,
        );
        let _ = writeln!(out, "  suggestion:");
        for line in suggestion.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    out
}

/// The disjunction the two SC#4 fixtures share, cut to `n` branches.
fn contact_disjunction(predicates: &[&str]) -> Rejection {
    Rejection::Disjunction {
        shape_iri: "http://example.org/Contact".to_string(),
        disjuncts: predicates.iter().map(|p| vec![(*p).to_string()]).collect(),
    }
}

// ===== Bucket 1: Forward Propagation (SC#1) — Db-wired ======================

#[test]
fn forward_propagation_typo_with_did_you_mean() {
    expect_file!["snapshots/forward_propagation_typo_with_did_you_mean.txt"].assert_eq(
        &run_db_wired_fixture("forward_propagation", "typo_with_did_you_mean"),
    );
}

// ===== Bucket 2: Backward Check (SC#2) — helper-proven ======================

/// A required, unbounded `email` constrained to `String` against a source
/// column typed `Optional<String>` — the cardinality blame. The constraint
/// below carries the predicate IRI and a `Primitive`; there is no CURIE in the
/// vocabulary it is built in.
#[test]
fn backward_check_optional_for_required() {
    let document = shape(
        "https://example.org/Person",
        vec![constraint(
            "https://example.org/email",
            Some(Primitive::STRING),
            Occurs { min: 1, max: None },
        )],
    );
    expect_file!["snapshots/backward_check_optional_for_required.txt"]
        .assert_eq(&run_shape_fixture("backward_check", &document, &[]));
}

/// An `age` narrowed to `Integer` against a source column typed `String` — the
/// type blame, and the constraint the document DID narrow.
#[test]
fn backward_check_type_mismatch_two_span() {
    let document = shape(
        "https://example.org/Person",
        vec![constraint(
            "https://example.org/age",
            Some(Primitive::Integer),
            Occurs::ONE,
        )],
    );
    expect_file!["snapshots/backward_check_type_mismatch_two_span.txt"]
        .assert_eq(&run_shape_fixture("backward_check", &document, &[]));
}

/// A predicate the document mentions and does not narrow. It renders `(any)`,
/// and `(any)` is what the checker enforces — nothing, rather than the
/// narrowest type in the lattice.
#[test]
fn backward_check_an_un_narrowed_predicate_expects_nothing() {
    let document = shape(
        "https://example.org/Person",
        vec![constraint("https://example.org/name", None, Occurs::ONE)],
    );
    expect_file!["snapshots/backward_check_an_un_narrowed_predicate_expects_nothing.txt"]
        .assert_eq(&run_shape_fixture("backward_check", &document, &[]));
}

// ===== Bucket 3: Value-disjunction rejection (SC#4) — helper-proven =========

#[test]
fn disjunction_rejection_two_branches() {
    let document = shape("http://example.org/Contact", Vec::new());
    let rejection = contact_disjunction(&["http://example.org/email", "http://example.org/phone"]);
    expect_file!["snapshots/disjunction_rejection_two_branches.txt"].assert_eq(&run_shape_fixture(
        "disjunction_rejection",
        &document,
        &[rejection],
    ));
}

#[test]
fn disjunction_rejection_three_branches() {
    let document = shape("http://example.org/Contact", Vec::new());
    let rejection = contact_disjunction(&[
        "http://example.org/email",
        "http://example.org/phone",
        "http://example.org/fax",
    ]);
    expect_file!["snapshots/disjunction_rejection_three_branches.txt"].assert_eq(
        &run_shape_fixture("disjunction_rejection", &document, &[rejection]),
    );
}

// ===== Bucket 4: Implicit Closure Synthesis (diagnostic shape) ==============
// This fixture locks did-you-mean firing on a qualified body column
// reference, `users.aeg` — the reference a synthesised closure would hold once
// a surface lambda form exists. Whether closure synthesis is reachable through
// a `.fossil` source is not measured here.

#[test]
fn implicit_closure_synthesis_field_typo() {
    expect_file!["snapshots/implicit_closure_synthesis_field_typo.txt"].assert_eq(
        &run_db_wired_fixture("implicit_closure_synthesis", "field_typo"),
    );
}

// ===== Bucket 5: did-you-mean threshold edges — Db-wired =====================

#[test]
fn did_you_mean_short_name_one_char() {
    // 2-char column `id`, typo `ig` → DL distance 1, threshold max(2, 2/3) = 2 → match.
    expect_file!["snapshots/did_you_mean_short_name_one_char.txt"]
        .assert_eq(&run_db_wired_fixture("did_you_mean", "short_name_one_char"));
}

#[test]
fn did_you_mean_unrelated_no_suggestion() {
    // Typo too far from any candidate → no "did you mean" in the diagnostic.
    expect_file!["snapshots/did_you_mean_unrelated_no_suggestion.txt"].assert_eq(
        &run_db_wired_fixture("did_you_mean", "unrelated_no_suggestion"),
    );
}

// ===== SC#4 second-order check: the generated split suggestion compiles ======

/// The suggestion is Fossil source the compiler emits, so the compiler has to
/// accept it back — every line of it. A body that kept only its `@subject =`
/// line would still be non-empty with every generated property dropped, so the
/// `properties().len()` assertion below is what closes that.
///
/// A property key is the bare last segment of a predicate IRI, and what puts a
/// shape name in scope is a `type { … } := io.shex("…")` binding over a
/// registered document, which is the preamble this test wraps the suggestion
/// in.
/// A branch whose predicates the decoder could not name gets a line telling the
/// author to fill it in, and that line is a fossil comment: `# TODO:` is
/// refused by the lexer, so applying the quick fix would leave a file that does
/// not parse.
#[test]
fn a_split_branch_with_no_predicate_still_parses() {
    let suggestion = render_split_suggestion(
        "Contact",
        "Contact",
        "users",
        "\"https://example.org/u/{users.id}\"",
        &[vec!["http://example.org/email".to_string()], vec![]],
    );
    let full = format!(
        "type {{ Contact }} := io.shex(\"contact.shex\")\nusers := io.csv(\"users.csv\")\n{suggestion}"
    );
    let db = new_db();
    let file = SourceFile::new(&db, full.clone(), "split.fossil".to_string());
    let errors = fossil_syntax::parse(&db, file).diagnostics(&db);
    assert!(
        errors.is_empty(),
        "the split did not parse: {errors:?}\n{full}"
    );
    assert_eq!(
        def_map(&db, file).mappings(&db).len(),
        2,
        "one mapping per branch:\n{full}"
    );
}

#[test]
fn the_generated_split_suggestion_compiles() {
    // 1. Render the split-into-N-mappings suggestion through the same function
    //    the checker stores in `Diagnostic.suggestion_source` (Blocker #3 — a
    //    typed carrier, NOT Markdown string parsing).
    let suggestion = render_split_suggestion(
        "Contact",
        "Contact",
        "users",
        "\"https://example.org/u/{users.id}\"",
        &[
            vec!["http://example.org/email".to_string()],
            vec!["http://example.org/phone".to_string()],
        ],
    );

    // 2. The generated split names a shape and reads a source. Prepend the
    //    binding for each so the snippet is a complete, parseable Fossil
    //    document. (The split text itself is the body the suggestion guarantees
    //    compiles.)
    let mut full = String::new();
    full.push_str("type { Contact } := io.shex(\"contact.shex\")\n");
    full.push_str("users := io.csv(\"users.csv\")\n");
    full.push_str(&suggestion);

    // 3. Parse + lower + type-check the generated split through the production
    //    pipeline. The document declares the two predicates the branches split
    //    on, so `email` and `phone` are keys a body may write; the row the
    //    mapping reads is registered the way a host registers one.
    let mut db = new_db();
    let file = SourceFile::new(&db, full.clone(), "split-suggestion.fossil".to_string());
    register_document(
        &mut db,
        "contact.shex",
        // `0 1`, not `1 1`: a split writes ONE branch per mapping, so a
        // required `phone` would make every `Contact1` incomplete and the
        // failure would be the fixture's, not the renderer's.
        "shape http://example.org/Contact\n\
         prop http://example.org/email string 0 1\n\
         prop http://example.org/phone string 0 1\n",
    );
    fossil_base::test_support::register_inferred(
        &mut db,
        "users.csv",
        &[
            ("id", Primitive::Integer),
            ("email", Primitive::STRING),
            ("phone", Primitive::STRING),
        ],
    );

    // The split must lower to one mapping per branch (proves it is
    // syntactically valid Fossil that the parser + lowering accept).
    let mappings = def_map(&db, file).mappings(&db).clone();
    assert_eq!(
        mappings.len(),
        2,
        "the generated split must parse into one Fossil mapping per branch; \
         got {} from:\n{full}",
        mappings.len()
    );

    for mloc in &mappings {
        let result = typecheck_mapping(&db, *mloc);
        let diags = read_by_check(&db, *mloc);
        assert!(
            result.checked(&db).is_ok(),
            "split-suggestion mapping #{} failed to type-check: {:#?}",
            mloc.index(&db),
            diags
        );
        // Every line the suggestion writes has to survive the lowering. A
        // property that vanishes here is a suggestion that "compiles" and
        // produces nothing — the exact silent loss this corpus exists to catch.
        let props = body(&db, *mloc).properties(&db).clone();
        assert_eq!(
            props.len(),
            2,
            "mapping #{} must carry its `@subject =` line AND its predicate \
             line; got {props:#?}",
            mloc.index(&db),
        );
        let body_diags: Vec<&Diagnostic> = body(&db, *mloc)
            .diagnostics(&db)
            .iter()
            .chain(fossil_syntax::parse(&db, file).diagnostics(&db))
            .collect();
        assert!(
            body_diags.is_empty(),
            "the generated split must lower without complaint: {body_diags:#?}"
        );
    }
}

// ===== Guard: the harness can actually resolve a shape document =============

/// Both locks that would make a `.shex` next to a fixture invisible, asserted
/// separately, because either one alone is enough and they fail identically.
///
/// # What this proves
///
/// 1. `new_db()` installs a row named `shex` that reads types.
///    `NativeSystem::providers` is the trait default — data rows only — so
///    under it `shape_document` answers `None` for every document registered.
/// 2. `register_shape_documents` puts the file under the key `file_at` looks
///    it up by — the document path joined onto the PROGRAM's directory. The
///    registry is a Salsa input; the disk is not consulted at any point.
///
/// # What it CANNOT prove
///
/// That the checker resolves a document end-to-end. It asserts that the harness
/// is not the reason it cannot. The other half — a fixture that NAMES one —
/// is the corpus: every Db-wired fixture carries a `person.shex` beside it and a
/// `type { … } := io.shex("…")` line above the mapping, and the snapshots show
/// what a resolved shape does to the diagnostics: it removes them.
///
/// It also cannot prove the decoder is a real one. It is not: the line format
/// exists so this crate can test against a resolved shape without naming a
/// schema language.
#[test]
fn the_harness_installs_a_decoder_and_registers_the_document() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diagnostics/harness");
    let key = dir.join("person.shex").to_string_lossy().to_string();

    let mut db = new_db();
    register_shape_documents(&mut db, &dir);

    let doc = fossil_base::file_at(&db, &key).unwrap_or_else(|| {
        panic!(
            "the fixture's shape document is not in the Salsa registry under \
             {key}: `decoded_document` reads `file_at`, never the disk, so a \
             `.shex` that is merely ON DISK resolves to nothing — and the \
             message for that is the one a program with no `type` line gets"
        )
    });
    let shapes = fossil_hir::shape_documents::shape_document(&db, doc, "shex").expect(
        "no installed row is named `shex`: `System::providers` defaults to the \
         DATA rows and `NativeSystem` never overrides it, so no row reads types \
         at all and every document decodes to nothing — silently, and \
         indistinguishably from a program that named none",
    );
    assert!(
        shapes.lookup("http://example.org/Person").is_some(),
        "the decoded document must declare the shape it names"
    );
}

// ===== Guard: no Unknown / InferenceId leak in any shape rendering ==========

#[test]
fn resolved_constraints_never_leak_unknown_or_inferenceid() {
    // The value-type rendering routes through render_ty_kind, so no corpus
    // rendering can leak a `TyKind`'s Rust spelling.
    let documents = [
        shape(
            "https://example.org/Person",
            vec![
                constraint(
                    "https://example.org/email",
                    Some(Primitive::STRING),
                    Occurs { min: 1, max: None },
                ),
                constraint("https://example.org/name", None, Occurs::ONE),
            ],
        ),
        shape("http://example.org/Contact", Vec::new()),
    ];
    let rejection = contact_disjunction(&["http://example.org/email"]);
    for document in &documents {
        let rendered = run_shape_fixture("guard", document, std::slice::from_ref(&rejection));
        assert!(
            !rendered.contains("Unknown"),
            "rendering leaked `Unknown`:\n{rendered}"
        );
        assert!(
            !rendered.contains("InferenceId"),
            "rendering leaked `InferenceId`:\n{rendered}"
        );
    }
}

// ===== A collision names the repair, and the repair is a declaration ======

/// Two predicates whose last IRI segments coincide. Both become unwritable and
/// the checker says so — a property key is the bare last segment of a predicate
/// IRI, so two that collide leave no name a body could write — and the evidence for refusing to
/// disambiguate is FSharp.Data's own: PLDI 2016 §6.5 declares a criterion its
/// naming scheme then ignored, and a minor version renamed members and broke a
/// program in production.
const COLLIDING_DOCUMENT: &str = "\
shape http://example.org/Person
prop http://example.org/name string 1 1
prop http://xmlns.com/foaf/0.1/name string 0 1
";

const COLLIDING_PROGRAM: &str = "\
type { Person } := io.shex(\"colliding.shex\")
User := io.csv(\"u.csv\")

People : Person from User
    @subject = \"http://example.org/p/{User.id}\"
    name = User.name
";

/// The collision is reported, and its help sends the author to the repair the
/// language has: declaring the type, where a field's name sits beside its IRI.
/// It names the bound type and the second IRI, and proposes a name for it.
#[test]
fn a_collision_names_the_declaration_that_repairs_it() {
    let mut db = new_db();
    let file = SourceFile::new(
        &db,
        COLLIDING_PROGRAM.to_string(),
        "colliding.fossil".to_string(),
    );
    register_document(&mut db, "colliding.shex", COLLIDING_DOCUMENT);
    let mapping = def_map(&db, file).mappings(&db)[0];
    let diagnostics = read_by_check(&db, mapping);
    let collision = diagnostics
        .iter()
        .find(|d| d.message().contains("both called `name`"))
        .unwrap_or_else(|| panic!("the two predicates must collide; got {diagnostics:#?}"));
    let help = collision.help.clone().expect("a collision says what to do");
    assert!(
        help.contains("Declare `Person` in the program")
            && help.contains("`foaf_name`")
            && help.contains("http://xmlns.com/foaf/0.1/name"),
        "the help names the type to declare, a field name and its IRI: {help}"
    );
}
