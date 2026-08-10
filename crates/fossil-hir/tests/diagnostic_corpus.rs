//! Phase 3 diagnostic golden-file corpus (plan 03-08).
//!
//! Locks SC#1..SC#5 of the Phase 3 ROADMAP success criteria as regression
//! tests. Grows in Phase 4-6 toward the 30+ target per ROADMAP
//! §"Cross-Phase Concerns" ("Negative-test suite for silent semantic errors
//! (each P-CRIT-4 pattern)").
//!
//! # Fixture layout
//!
//! Each fixture lives under `tests/fixtures/diagnostics/<bucket>/<name>/` with:
//!   - `mapping.fossil` — required (the source).
//!   - `descriptor.csvw.json` — optional CSVW input descriptor (resolved by the
//!     production `resolve_source_row` path via `schema = "..."`).
//!   - `shape.shex` — optional ShEx target schema (ShExC JSON form).
//!
//! # Driving strategy — "Db-wired" vs "helper-proven"
//!
//! The production `typecheck_mapping` Salsa query reaches the source row
//! (CSVW) through `db.system().read_file(...)`, so forward CSVW propagation
//! (SC#1) is exercised **end-to-end through the production query path**:
//! `run_csvw_fixture` writes a real `SourceFile` whose path lets the relative
//! `schema = "..."` argument resolve, then drains the diagnostics the query
//! accumulated. These fixtures are labelled **Db-wired**.
//!
//! Backward ShEx checking (SC#2) and ShEx OneOf rejection (SC#4) are NOT driven
//! end-to-end here: these fixtures name no output shape document, so
//! `resolve_target_shape` returns `None` for them (the end-to-end path has its
//! own fixtures under `tests/fixtures/output_shape/`). So these buckets drive
//! the **same plain-Rust descriptor logic the production checker uses** —
//! `ShExDescriptor::from_reader` →
//! `ResolvedShape::from_binding` → `one_of_rejections` /
//! `generate_split_suggestion` — directly over the fixture's `shape.shex`.
//! These fixtures are labelled **helper-proven**. The `Checker` struct itself
//! has `pub(crate)` fields and is not constructible from an integration test,
//! so we exercise its inputs and the descriptor lowering it consumes, which is
//! exactly the logic SC#2/SC#4 assert.
//!
//! Every snapshot header records which mode the fixture used so the
//! test-helper-vs-Db-wired status is auditable in the committed `.snap` files.

// `doc_markdown`: the doc comments name bare SC identifiers (SC#1, ShEx, OneOf,
// AcceptAll, Db) that read naturally without backticks in this test harness.
// `literal_string_with_formatting_args`: the Fossil IRI template
// `${ex:}u/${.id}` is LITERAL Fossil source passed to the suggestion generator,
// not a Rust format string (the workspace allows this elsewhere — see
// check.rs / shex.rs).
#![allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]

use std::path::Path;
use std::path::PathBuf;

use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_output::{ShExDescriptor, ShExLoweringError, generate_split_suggestion};
use fossil_hir::body::body;
use fossil_hir::def_map::def_map;
use fossil_hir::shapes::{ResolvedShape, one_of_rejections};
use fossil_hir::ty::ShapeId;
use fossil_hir::{render_ty_kind, typecheck_mapping};
use insta::assert_snapshot;
use rudof_iri::IriS;
use std::sync::Arc;

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

fn new_db() -> FossilDb {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    FossilDb::new(system)
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
fn render_diagnostic(out: &mut String, diag: &Diagnostic) {
    use std::fmt::Write as _;
    let _ = writeln!(out, "{:?}: {}", diag.severity, redact_paths(&diag.message));
    let _ = writeln!(out, "  at {}..{}", diag.span.start, diag.span.end);
    if let Some(sugg) = &diag.suggestion_source {
        let _ = writeln!(out, "  suggestion:");
        for line in sugg.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
}

// ---------------------------------------------------------------------------
// CSVW forward-propagation driver (Db-wired — production query path)
// ---------------------------------------------------------------------------

/// Drive a CSVW-forward fixture **end-to-end through the production
/// `typecheck_mapping` query**. The `mapping.fossil`'s `schema = "..."` arg is
/// resolved relative to the fixture directory (via `db.system().read_file`),
/// so the source row is built by the real `resolve_source_row` path.
///
/// Returns the rendered diagnostics (the golden output the `.snap` captures).
fn run_csvw_fixture(bucket: &str, name: &str) -> String {
    let dir = fixture_dir(bucket, name);
    let src = read_fixture(&dir, "mapping.fossil")
        .unwrap_or_else(|| panic!("fixture {bucket}/{name} must have a mapping.fossil"));

    let db = new_db();
    // The SourceFile path makes the relative `schema = "..."` arg resolve
    // against the fixture directory (resolve_relative joins on the parent dir).
    let file_path = dir.join("mapping.fossil");
    let file = SourceFile::new(&db, src, file_path.to_string_lossy().to_string());

    let mut out = String::new();
    out.push_str("# mode: Db-wired (production typecheck_mapping query)\n");
    out.push_str("# bucket: ");
    out.push_str(bucket);
    out.push('\n');

    let mappings = def_map(&db, file).mappings(&db).clone();
    let mut all_diags: Vec<Diagnostic> = Vec::new();
    for mloc in &mappings {
        let _ = typecheck_mapping(&db, *mloc);
        all_diags.extend(
            typecheck_mapping::accumulated::<Diagnostic>(&db, *mloc)
                .into_iter()
                .cloned(),
        );
    }

    if all_diags.is_empty() {
        out.push_str("(no diagnostics emitted)\n");
    } else {
        // Stable ordering: by span then message.
        all_diags.sort_by(|a, b| {
            (a.span.start, a.span.end, a.message.as_str()).cmp(&(
                b.span.start,
                b.span.end,
                b.message.as_str(),
            ))
        });
        for diag in &all_diags {
            render_diagnostic(&mut out, diag);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// ShEx backward-check + OneOf driver (helper-proven — descriptor logic)
// ---------------------------------------------------------------------------

/// Drive a ShEx fixture via the plain-Rust descriptor logic the production
/// checker consumes (`ShExDescriptor::from_reader` → `ResolvedShape`). Renders
/// the resolved constraint table + any lowering errors + the generated split
/// suggestion (for OneOf), so the `.snap` locks the SC#2/SC#4 surface.
fn run_shex_fixture(bucket: &str, name: &str) -> String {
    use std::fmt::Write as _;
    let dir = fixture_dir(bucket, name);
    let shex_src = read_fixture(&dir, "shape.shex")
        .unwrap_or_else(|| panic!("fixture {bucket}/{name} must have a shape.shex"));

    let db = new_db();
    let descriptor = ShExDescriptor::from_reader(shex_src.as_bytes())
        .unwrap_or_else(|e| panic!("shape.shex for {bucket}/{name} failed to parse: {e:?}"));
    let errors = descriptor.lowering_errors().to_vec();

    let mut out = String::new();
    out.push_str("# mode: helper-proven (ShExDescriptor + ResolvedShape; resolve_target_shape\n");
    out.push_str(
        "#       returns None in the production Db path until Phase 6 — see module docs)\n",
    );
    out.push_str("# bucket: ");
    out.push_str(bucket);
    out.push('\n');

    // Resolved constraint table (the SC#2 backward-check input surface).
    let binding = descriptor.shapes().next();
    if let Some(binding) = binding {
        let shape =
            ResolvedShape::from_binding(&db, binding, ShapeId::placeholder(0), errors.clone());
        if shape.constraints.is_empty() {
            let _ = writeln!(
                out,
                "resolved constraints: (none — OneOf body, see rejection below)"
            );
        } else {
            let _ = writeln!(out, "resolved constraints:");
            for c in &shape.constraints {
                let ty_text = c
                    .value_ty
                    .map_or_else(|| "(any)".to_string(), |t| render_ty_kind(&db, t.kind(&db)));
                let _ = writeln!(out, "  {} : {} [{:?}]", c.predicate, ty_text, c.cardinality);
            }
        }
    } else {
        let _ = writeln!(out, "resolved constraints: (no IRI-identified shape)");
    }

    // Lowering errors — OneOf rejection carries the generated split suggestion.
    let rejections = one_of_rejections(&errors);
    if rejections.is_empty() {
        let _ = writeln!(out, "lowering errors: (none)");
    } else {
        for rej in &rejections {
            let _ = writeln!(
                out,
                "OneOf rejection: {} disjuncts in shape `{}`",
                rej.disjunct_count, rej.shape_iri
            );
            let preds: Vec<String> = rej
                .disjunct_predicates
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            let _ = writeln!(out, "  disjunct predicates: {}", preds.join(", "));
            let suggestion = generate_split_suggestion(
                "Contact",
                "`${ex:}u/${.id}`",
                "users",
                "ex:Contact",
                &rej.suggestion_seed.one_of_node,
            );
            let _ = writeln!(out, "  suggestion:");
            for line in suggestion.lines() {
                let _ = writeln!(out, "    {line}");
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// AcceptAll degraded-fallback driver (SC#5 — Blocker #4)
// ---------------------------------------------------------------------------

/// SC#5 verification (Blocker #4): inject the `AcceptAll` descriptor path. The
/// fixture HAS a `shape.shex` file, but the production `typecheck_mapping`
/// query never reaches it (`resolve_target_shape` returns `None` — equivalent
/// to the host defaulting to `OutputDescriptorKind::AcceptAll`). So no
/// backward-check diagnostics fire and the mapping compiles equivalently to a
/// walking-skeleton case.
///
/// This proves the SC#5 plug-in-replaceability claim: the same `fossil-hir`
/// code path runs whether the host supplies `ShEx` or `AcceptAll` — only the
/// `OutputDescriptorKind` discriminant differs, and `AcceptAll` cleanly
/// degrades the diagnostic surface to "no backward check".
fn run_accept_all_fixture(bucket: &str, name: &str) -> String {
    use std::fmt::Write as _;
    let dir = fixture_dir(bucket, name);
    // Confirm the fixture DOES carry a shape.shex (which AcceptAll ignores).
    assert!(
        dir.join("shape.shex").exists(),
        "the accept_all fixture must carry a shape.shex that AcceptAll ignores"
    );
    let src = read_fixture(&dir, "mapping.fossil")
        .unwrap_or_else(|| panic!("fixture {bucket}/{name} must have a mapping.fossil"));

    let db = new_db();
    let file_path = dir.join("mapping.fossil");
    let file = SourceFile::new(&db, src, file_path.to_string_lossy().to_string());

    let mut out = String::new();
    out.push_str("# mode: SC#5 AcceptAll degraded fallback (Blocker #4)\n");
    out.push_str("# shape.shex PRESENT but ignored — resolve_target_shape == None == AcceptAll\n");

    let mappings = def_map(&db, file).mappings(&db).clone();
    let mut backward_diags = 0usize;
    let mut all_diags: Vec<Diagnostic> = Vec::new();
    for mloc in &mappings {
        let _ = typecheck_mapping(&db, *mloc);
        for d in typecheck_mapping::accumulated::<Diagnostic>(&db, *mloc) {
            // A backward-check diagnostic would mention the shape demand /
            // OneOf. None should appear under AcceptAll.
            if d.message.contains("target shape") || d.message.contains("OneOf") {
                backward_diags += 1;
            }
            all_diags.push(d.clone());
        }
    }

    let _ = writeln!(out, "backward-check diagnostics: {backward_diags}");
    if all_diags.is_empty() {
        out.push_str("(no diagnostics emitted — compiles equivalently to walking-skeleton)\n");
    } else {
        all_diags.sort_by(|a, b| {
            (a.span.start, a.span.end, a.message.as_str()).cmp(&(
                b.span.start,
                b.span.end,
                b.message.as_str(),
            ))
        });
        for diag in &all_diags {
            render_diagnostic(&mut out, diag);
        }
    }
    // SC#5 assertion: AcceptAll never produces backward-check diagnostics.
    assert_eq!(
        backward_diags, 0,
        "AcceptAll must skip backward checking (SC#5 plug-in-replaceability)"
    );
    out
}

// ===== Bucket 1: CSVW Forward Propagation (SC#1) — Db-wired =================

#[test]
fn csvw_forward_propagation_typo_with_did_you_mean() {
    assert_snapshot!(run_csvw_fixture(
        "csvw_forward_propagation",
        "typo_with_did_you_mean"
    ));
}

#[test]
fn csvw_forward_propagation_missing_descriptor() {
    assert_snapshot!(run_csvw_fixture(
        "csvw_forward_propagation",
        "missing_descriptor"
    ));
}

#[test]
fn csvw_forward_propagation_unknown_datatype() {
    assert_snapshot!(run_csvw_fixture(
        "csvw_forward_propagation",
        "unknown_datatype"
    ));
}

// ===== Bucket 2: ShEx Backward Check (SC#2) — helper-proven =================

#[test]
fn shex_backward_check_optional_for_required() {
    assert_snapshot!(run_shex_fixture(
        "shex_backward_check",
        "optional_for_required"
    ));
}

#[test]
fn shex_backward_check_type_mismatch_two_span() {
    assert_snapshot!(run_shex_fixture(
        "shex_backward_check",
        "type_mismatch_two_span"
    ));
}

// ===== Bucket 3: ShEx OneOf Rejection (SC#4) — helper-proven ================

#[test]
fn shex_one_of_rejection_two_disjuncts() {
    assert_snapshot!(run_shex_fixture("shex_one_of_rejection", "two_disjuncts"));
}

#[test]
fn shex_one_of_rejection_three_disjuncts() {
    assert_snapshot!(run_shex_fixture("shex_one_of_rejection", "three_disjuncts"));
}

// ===== Bucket 4: Implicit Closure Synthesis (SC#3 — diagnostic shape) =======
// Phase 3 v0.1 has NO surface pipeline/closure form (HirExpr is the Phase 2
// leaf surface — no Call/Pipeline), so implicit closure synthesis is unreachable
// through a .fossil source + typecheck_mapping. The closure-synthesis algorithm
// + SC#3 rendering are proven in-crate (crates/fossil-hir/src/check_tests.rs,
// plan 03-06) and at the LSP hover layer (crates/fossil-lsp/tests/
// lsp_hover_smoke.rs, plan 03-07). These corpus fixtures lock the reachable
// surface: did-you-mean firing on a body FieldRef that WILL live inside a
// synthesised closure once the surface lambda/pipeline form lands.

#[test]
fn implicit_closure_synthesis_field_typo() {
    assert_snapshot!(run_csvw_fixture("implicit_closure_synthesis", "field_typo"));
}

#[test]
fn implicit_closure_synthesis_type_mismatch_inside_body() {
    assert_snapshot!(run_csvw_fixture(
        "implicit_closure_synthesis",
        "type_mismatch_inside_body"
    ));
}

// ===== Bucket 5: did-you-mean threshold edges — Db-wired =====================

#[test]
fn did_you_mean_short_name_one_char() {
    // 2-char column `id`, typo `ig` → DL distance 1, threshold max(2, 2/3) = 2 → match.
    assert_snapshot!(run_csvw_fixture("did_you_mean", "short_name_one_char"));
}

#[test]
fn did_you_mean_unrelated_no_suggestion() {
    // Typo too far from any candidate → no "did you mean" in the diagnostic.
    assert_snapshot!(run_csvw_fixture("did_you_mean", "unrelated_no_suggestion"));
}

// ===== Bucket 6: AcceptAll degraded-fallback path (SC#5 — Blocker #4) ========

#[test]
fn accept_all_descriptor_skips_backward_check() {
    assert_snapshot!(run_accept_all_fixture(
        "accept_all",
        "descriptor_skips_backward_check"
    ));
}

// ===== SC#4 second-order check: the generated split suggestion compiles ======

/// Extract the structured `Diagnostic.suggestion_source` for the OneOf
/// rejection. We surface the rejection via the production checker's diagnostic
/// emitter shape (`ShExDescriptor` lowering errors → `generate_split_suggestion`,
/// the exact text plan 03-05 stores in `Diagnostic.suggestion_source`). Returns
/// the typed suggestion text — NOT a Markdown-message substring (Blocker #3).
fn extract_suggestion_from_fixture(bucket: &str, name: &str) -> Option<String> {
    let dir = fixture_dir(bucket, name);
    let shex_src = read_fixture(&dir, "shape.shex")?;
    let descriptor = ShExDescriptor::from_reader(shex_src.as_bytes()).ok()?;
    let errors = descriptor.lowering_errors().to_vec();
    let rej = errors.iter().find_map(|e| match e {
        ShExLoweringError::OneOfRejection(r) => Some(r.clone()),
        _ => None,
    })?;
    // The split suggestion text — this is what plan 03-05 puts into the
    // structured `Diagnostic.suggestion_source` field on the OneOf-rejection
    // diagnostic (see check.rs::surface_shape_lowering_errors).
    Some(generate_split_suggestion(
        "Contact",
        "`${ex:}u/${.id}`",
        "users",
        "ex:Contact",
        &rej.suggestion_seed.one_of_node,
    ))
}

#[test]
fn shex_one_of_split_suggestion_compiles() {
    // 1. Extract the generated split-into-N-mappings suggestion via the typed
    //    suggestion-source carrier (Blocker #3 — NOT Markdown string parsing).
    let suggestion = extract_suggestion_from_fixture("shex_one_of_rejection", "two_disjuncts")
        .expect("OneOf rejection fixture must yield a structured suggestion");

    // 2. The generated split references full IRI predicates + `.field` accesses.
    //    Prepend a prefix decl + a CSVW-described source so the snippet is a
    //    complete, parseable Fossil document. (The split text itself is the
    //    body the suggestion guarantees compiles.)
    let dir = fixture_dir("shex_one_of_rejection", "two_disjuncts");
    let mut full = String::new();
    full.push_str("prefix ex: <http://example.org/>\n");
    full.push_str("users := io.csv(\"users.csv\", schema = \"descriptor.csvw.json\")\n");
    full.push_str(&suggestion);

    // 3. Parse + lower + type-check the generated split through the production
    //    pipeline. Use the fixture dir as the file's home so the schema arg
    //    resolves (descriptor.csvw.json exists in two_disjuncts/).
    let db = new_db();
    let file_path = dir.join("split-suggestion.fossil");
    let file = SourceFile::new(&db, full, file_path.to_string_lossy().to_string());

    // The split must lower to ≥1 mapping (proves it is syntactically valid
    // Fossil that the parser + lowering accept).
    let mappings = def_map(&db, file).mappings(&db).clone();
    assert!(
        !mappings.is_empty(),
        "generated split suggestion must parse into ≥1 Fossil mapping; \
         got zero mappings from:\n{suggestion}"
    );

    // 4. Each generated mapping type-checks cleanly (the split bodies project
    //    real CSVW columns — id/email/phone — so no field-not-found fires).
    for mloc in &mappings {
        let result = typecheck_mapping(&db, *mloc);
        let diags = typecheck_mapping::accumulated::<Diagnostic>(&db, *mloc);
        assert!(
            result.is_ok(),
            "split-suggestion mapping #{} failed to type-check: {:#?}",
            mloc.index(&db),
            diags
        );
    }

    // 5. Sanity: the two disjunct predicates each became a mapping.
    let body0 = body(&db, mappings[0]);
    assert!(
        !body0.properties(&db).is_empty(),
        "first split mapping must carry properties"
    );
}

// ===== Guard: no Unknown / InferenceId leak in any descriptor rendering ======

#[test]
fn shex_resolved_constraints_never_leak_unknown_or_inferenceid() {
    // The ShEx value-type rendering routes through render_ty_kind, which
    // normalises TyKind::Unknown(InferenceId) to `?`. Assert no corpus ShEx
    // rendering leaks the internal placeholder (Risk Register / STATE.md "Do
    // NOT").
    for (bucket, name) in [
        ("shex_backward_check", "optional_for_required"),
        ("shex_backward_check", "type_mismatch_two_span"),
        ("shex_one_of_rejection", "two_disjuncts"),
        ("shex_one_of_rejection", "three_disjuncts"),
    ] {
        let rendered = run_shex_fixture(bucket, name);
        assert!(
            !rendered.contains("Unknown"),
            "{bucket}/{name} rendering leaked `Unknown`:\n{rendered}"
        );
        assert!(
            !rendered.contains("InferenceId"),
            "{bucket}/{name} rendering leaked `InferenceId`:\n{rendered}"
        );
    }
}

// Touch IriS so the import is used even if a future refactor drops the explicit
// reference; keeps the helper crate surface honest.
#[allow(dead_code)]
fn _iri_smoke() -> IriS {
    IriS::new_unchecked("http://example.org/")
}
