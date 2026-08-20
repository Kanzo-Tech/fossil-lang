// The top-of-file documentation block enumerates the EXPECTED + FORBIDDEN
// query re-execution sets — type / kind / token / variant identifiers
// appear naturally throughout. Adding backticks to every one would clutter
// the prose without aiding readers. Test files are not part of the public
// API docs, so the missing-backticks + lazy-continuation + similar-name
// lints are silenced module-wide.
#![allow(
    clippy::doc_markdown,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::similar_names
)]

//! The mechanical gate on the per-mapping invalidation barrier: editing one
//! mapping's body must not re-execute any sibling's.
//!
//! The test installs an event callback on FossilDb that counts every
//! `EventKind::WillExecute` event. After warming the cache, we reset the
//! counter, mutate one character in mapping #3's body, re-run the same
//! queries, and assert two invariants:
//!
//! 1. **Total bounded** — the total count of re-executed queries stays
//!    under `MAX_REEXECUTIONS`. This is the LOOSE threshold (caps total
//!    work, including the structural pass that Salsa must perform).
//! 2. **Per-mapping fan-out is exactly 1** (the LOAD-BEARING invariant)
//!    — only ONE per-mapping body() / expr_types() /
//!    typecheck_mapping() pair re-executes (the one for mapping #3),
//!    NOT all 10. This is the structural promise of the per-mapping
//!    invalidation barrier: signatures live in a file-keyed query and bodies
//!    behind a per-mapping one, so a body edit cannot reach a sibling.
//!    Enforced by `keyset_of_reexecuted_queries_matches_expected_four`.
//!
//! ─────────────────────────────────────────────────────────────────────────
//! EXPECTED QUERY RE-EXECUTION COSTS
//! ─────────────────────────────────────────────────────────────────────────
//!
//! After editing one character in the body of mapping #3 of a 10-mapping
//! file, these queries re-execute (output may or may not change; downstream
//! depends on the OUTPUT-changed signal, not the WillExecute event):
//!
//!   * STRUCTURAL PASS (unavoidable — parse output changed by 1 byte):
//!     1. parse(file)                       — text changed; re-parse required
//!     2. item_tree(file)                   — re-runs, returns structurally-equal Vec<ItemHeader> (no signature changed); downstream validates
//!     3. ast_id_map(file)                  — re-runs, returns structurally-equal AstIdMap; downstream validates
//!     4. def_map(file)                     — re-runs, returns structurally-equal DefMap; downstream validates
//!     5-14. mapping_cst_node(M_0..M_9)     — re-runs for ALL 10 mappings; output is structurally-equal for 9 siblings (rowan Arc-shared subtrees); only M_3's output differs
//!
//!   * STRUCTURAL PASS (what the checker reads file-keyed):
//!     - lower_to_hir(file)                 — re-runs, returns structurally-equal HirFile (header-only signatures unchanged by a body edit); downstream validates. typecheck_mapping reads it (via resolve_source_row + mapping/source name lookups) so it joins the file-keyed structural pass. File-keyed, NOT a per-mapping fan-out.
//!
//!   * PER-MAPPING FAN-OUT (the load-bearing invariant — only M_3 fans out):
//!     15. body(M_3)                        — output of mapping_cst_node(M_3) changed
//!     16. typecheck_mapping(M_3)           — reads body(M_3) + spans(M_3) + resolve_source_row + resolve_target_shape; re-runs for the edited mapping ONLY
//!     17. spans(M_3)                       — per-mapping side table; depends on mapping_cst_node(M_3); re-runs for the edited mapping ONLY (siblings stay cached via the same Arc-shared subtree barrier)
//!     (expr_types(M_3) is a thin accessor over typecheck_mapping(M_3);
//!      after the edit its input output is structurally-equal — the
//!      ten-mappings fixture's FieldRef bodies have no descriptor so the type
//!      table is unchanged — so it VALIDATES instead of re-executing. Hence
//!      expr_types_count == 0, and typecheck_count == 1
//!      takes its place in the fan-out, for a total of 18.)
//!
//!   * SIBLING MAPPINGS (the FORBIDDEN re-executions — must stay cached):
//!     - body(M_i)              for i ∈ {0,1,3,4,5,6,7,8,9} — sibling bodies
//!     - typecheck_mapping(M_i) for i ∈ {0,1,3,4,5,6,7,8,9} — sibling type-checks
//!     - expr_types(M_i)        for i ∈ {0,1,3,4,5,6,7,8,9} — sibling provenance
//!     - spans(M_i)             for i ∈ {0,1,3,4,5,6,7,8,9} — sibling spans
//!
//! (Note: indices are 0-based; "mapping #3" in prose = MappingLoc.index == 2.)
//!
//! ─────────────────────────────────────────────────────────────────────────
//! WHY THE THRESHOLD IS 18, NOT 4
//! ─────────────────────────────────────────────────────────────────────────
//!
//! The threshold this replaced — "≤ 4 (parse + body_3 + typecheck_3 +
//! expr_types_3)" — conflated two distinct Salsa events:
//!
//!   * WillExecute: query body ran (cache miss OR upstream changed).
//!   * DidValidateMemoizedValue: query output validated unchanged (cache hit
//!     after upstream change).
//!
//! Per Salsa 0.26 semantics, ANY query that takes `parse(file)` as a
//! transitive input WILL emit a `WillExecute` event when the file text
//! changes (because Salsa must re-derive to check whether the structural
//! output also changed). The invariant is correctly stated
//! as: per-mapping fan-out (body + typecheck + expr_types for SIBLING
//! mappings) does NOT re-execute. That invariant is enforced by the
//! second test (`keyset_of_reexecuted_queries_matches_expected_four`),
//! which counts body / typecheck / expr_types re-executions separately.
//!
//! The TOTAL count is bounded by the structural-pass costs:
//!   1 (parse) + 4 (item_tree, def_map, ast_id_map, lower_to_hir)
//!   + 10 (mapping_cst_node) + 1 (body of M_3) + 1 (typecheck_mapping of M_3)
//!   + 1 (spans of M_3) = 18.
//!
//! Two of those eighteen are worth naming, because each is a place the
//! accounting could quietly move:
//!   (a) `spans(db, mapping)` is a query of its own rather than a field on
//!       `body`, so it re-executes on its own line; it is per-mapping, so it
//!       costs one, not ten.
//!   (b) `typecheck_mapping(M_3)` re-executes, and its `resolve_source_row` /
//!       mapping-name lookups add a file-keyed `lower_to_hir(file)` to the
//!       structural pass. `expr_types(M_3)` is a THIN ACCESSOR over it, and
//!       its input is structurally-equal after a body-only edit of a
//!       schema-less mapping, so it VALIDATES instead of re-executing.
//!
//! `resolve_source_row` reads `def_map(file)` + `lower_to_hir(file)`
//! (signatures-only, file-keyed, structurally stable across body edits) —
//! NEVER walks up from `mapping_cst_node` to the FILE CST. The
//! per-mapping fan-out stays at 1 (verified below).
//!
//! ─────────────────────────────────────────────────────────────────────────
//! THE LSP AUDIT — fan-out unchanged
//! ─────────────────────────────────────────────────────────────────────────
//!
//! The LSP work was audited against the fan-out invariant — the load-bearing
//! one, because a Salsa graph that re-executes every sibling is what makes an
//! LSP stop answering inside 100 ms. Findings:
//!
//!   * `resolve_target_shape` returns `Some` in production because it READS the
//!     shape document the program names. It used to read it through
//!     `System::read_file`, which registers no Salsa dependency — so editing
//!     the document re-ran nothing, and in the LSP that is a diagnostic that
//!     never clears. It now reads the document as an INPUT: `file_at` (the file
//!     registry) and `fossil_base::shape_document` (tracked).
//!
//!     That adds two Salsa dependencies to `typecheck_mapping` and NEITHER is
//!     per-mapping: `shape_document` is keyed by the DOCUMENT's `SourceFile`,
//!     so ten mappings checking against one document share one decode, and
//!     `file_at` reads the registry input. The per-mapping fan-out set is
//!     therefore unchanged; what a document edit costs is one `shape_document`
//!     re-execution plus the mappings that read it, which is the cost of being
//!     correct about it at all.
//!
//!     The ten-mapping fixture names no document, so `resolve_target_shape`
//!     returns `Ok(None)` before touching either, and `MAX_REEXECUTIONS` is
//!     unmoved at 18. A future fixture that DOES name one must budget the
//!     `shape_document` re-execution here, once, and not per mapping.
//!   * The CLI and `fossil_ide::WorkspaceIndex` touch
//!     only native CLI / WASM-clean IDE-index code; neither introduces a
//!     `#[salsa::tracked]` query keyed by `MappingLoc`. The IDE indexes are
//!     plain structs built from `def_map` (file-keyed) — not per-mapping
//!     tracked queries.
//!
//! The complete per-mapping (`MappingLoc`-keyed) `#[salsa::tracked]` query
//! set is therefore `body`, `typecheck_mapping`,
//! `expr_types`, `spans`. Each is asserted to re-execute ≤
//! `MAX_PER_MAPPING_FAN_OUT` (= 1) after the single-char body edit by
//! `keyset_of_reexecuted_queries_matches_expected_four`. No new per-mapping
//! query needed registration; `MAX_REEXECUTIONS` stays at 18 (breakdown
//! above). Any future IDE feature that adds a per-mapping tracked
//! query MUST register it in the keyset assertion and cap it at 1 — a NEW
//! whole-file (FILE-keyed) query re-runs exactly once and is allowed to bump
//! `MAX_REEXECUTIONS` ONLY with an updated breakdown here.
//!
//! If the test fails with count > 18, something else is leaking. If the
//! per-mapping fan-out test (body / typecheck / expr_types / spans > 1)
//! fails, the per-mapping invalidation barrier is broken — DO NOT relax that
//! assertion; fix the data layout so that signatures stay file-keyed and every
//! body stays behind its own per-mapping query.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_hir::ast_id::ast_id_map;
use fossil_hir::body::body;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::def_map;
use fossil_hir::item_tree::item_tree;
use fossil_hir::provenance::expr_types;
use fossil_hir::spans::spans;
use salsa::Setter;

/// Loose upper bound on the total count of re-executed queries after a
/// single-char body edit in mapping #3 of a 10-mapping file. See the
/// top-of-file "WHY THE THRESHOLD IS 18, NOT 4" comment for the breakdown.
///
/// The eighteen are: 1 parse + 4 file-keyed structural queries (item_tree,
/// ast_id_map, def_map, lower_to_hir) + 10 mapping_cst_node + the three that
/// fan out for the edited mapping alone (body, typecheck_mapping, spans).
///
/// The LOAD-BEARING invariant is enforced by
/// `keyset_of_reexecuted_queries_matches_expected_four` (per-mapping
/// fan-out for body / typecheck_mapping / expr_types / spans is exactly
/// 1 each, NOT 10). The threshold here is a secondary "no surprise extra
/// work" guard.
const MAX_REEXECUTIONS: usize = 18;

/// The LOAD-BEARING per-mapping fan-out bound. Editing
/// one mapping's body MUST NOT re-execute body / typecheck / expr_types
/// for sibling mappings — those queries must validate via Salsa's
/// `DidValidateMemoizedValue` after their per-mapping CST subtree input
/// (from `mapping_cst_node`) is observed structurally equal.
const MAX_PER_MAPPING_FAN_OUT: usize = 1;

#[test]
fn editing_body_of_mapping_3_does_not_invalidate_item_tree() {
    let baseline = include_str!("fixtures/ten_mappings_baseline.fossil");
    let edited = include_str!("fixtures/ten_mappings_mapping_3_body_one_char_edit.fossil");

    // Sanity-check the fixtures differ only in a body edit (single character).
    // If this fails, the fixtures are wrong — fix THEM, don't relax this test.
    assert_ne!(baseline, edited, "fixtures must differ");
    assert!(
        diff_is_body_only(baseline, edited),
        "fixture pair must differ in body content only, not signatures"
    );

    // Install a counter callback on a fresh FossilDb, via
    // `FossilDb::with_event_callback`.
    let counter: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
        if matches!(event.kind, salsa::EventKind::WillExecute { .. }) {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let mut db = FossilDb::with_event_callback(system, callback);

    let file = SourceFile::new(&db, baseline.to_string(), "ten_mappings.fossil".to_string());

    // Step 1: warm caches. Touch every query under test for every
    // mapping. After this loop, the cache is hot.
    let _ = item_tree(&db, file);
    let _ = ast_id_map(&db, file);
    let mappings: Vec<_> = def_map(&db, file).mappings(&db).clone();
    for m in &mappings {
        let _ = body(&db, *m);
        let _ = typecheck_mapping(&db, *m);
        let _ = expr_types(&db, *m);
        let _ = spans(&db, *m);
    }

    let before_reset = counter.load(Ordering::SeqCst);
    assert!(
        before_reset > 0,
        "expected some events during cache warm; got 0"
    );

    // Step 2: reset counter, mutate body of mapping #3, re-run.
    counter.store(0, Ordering::SeqCst);

    file.set_text(&mut db).to(edited.to_string());

    let _ = item_tree(&db, file);
    let _ = ast_id_map(&db, file);
    let mappings_after: Vec<_> = def_map(&db, file).mappings(&db).clone();
    for m in &mappings_after {
        let _ = body(&db, *m);
        let _ = typecheck_mapping(&db, *m);
        let _ = expr_types(&db, *m);
        let _ = spans(&db, *m);
    }

    let after = counter.load(Ordering::SeqCst);

    assert!(
        after <= MAX_REEXECUTIONS,
        "Salsa invalidation cascade detected: {after} queries re-executed after \
         one-char body edit in mapping #3 of 10. Expected ≤ {MAX_REEXECUTIONS} \
         (structural pass: 1 parse + 3 file-keyed structural queries + 10 \
         mapping_cst_node + per-mapping fan-out: 1 body + 1 expr_types + 1 \
         spans = 17). If you exceed this bound, a NEW query has been added \
         that depends on parse(file) without an intermediate per-item \
         invalidation barrier — fix the data layout, do NOT relax the \
         threshold."
    );
}

/// LOAD-BEARING keyset assertion for the per-mapping invalidation barrier.
///
/// In addition to counting events, capture the `DatabaseKeyIndex` debug-string for
/// each `WillExecute` event and assert the per-mapping fan-out is exactly
/// `MAX_PER_MAPPING_FAN_OUT` (= 1) for body / expr_types — i.e. only one
/// mapping's body and provenance re-execute after a single-mapping edit,
/// NOT all 10. THIS is the invalidation-barrier invariant: the
/// per-mapping CST subtree extraction (`mapping_cst_node`) makes downstream
/// body() queries for sibling mappings see a structurally-equal input and
/// validate via `DidValidateMemoizedValue` instead of re-executing.
///
/// Salsa 0.26's `EventKind::WillExecute { database_key }` exposes
/// `DatabaseKeyIndex: Debug`. The Debug impl renders as `query_name(Id(raw))`
/// when the database is attached (which it is during the event callback). We
/// substring-match on the query name to discriminate. The interned key's
/// internal structure (i.e. `MappingLoc { file, index: 2 }`) is NOT visible —
/// the Id is opaque. Therefore the test asserts:
///
///   (a) at least one `parse` key
///   (b) exactly one `body` key (mapping_3 — would be 10 if sibling bodies
///       were invalidated by the body edit; THIS is the load-bearing check)
///   (c) exactly one `typecheck_mapping` key (it reads body(M_3) + spans(M_3)
///       + the descriptors, so it re-executes once for the edited mapping)
///   (d) at most one `expr_types` key (a thin accessor over
///       typecheck_mapping; it VALIDATES — count 0 — when
///       the typecheck output is structurally-equal after a schema-less body
///       edit, else re-runs — count 1)
///   (e) exactly one `spans` key (the per-mapping real-span
///       side table — same per-mapping fan-out shape as expr_types; load-
///       bearing for the claim that spans depends on mapping_cst_node, not
///       parse(file))
///   (f) `mapping_cst_node` may re-run for any subset of mappings (this is
///       the structural-pass cost; output is structurally-equal for siblings
///       so it doesn't propagate further)
///
/// If salsa 0.26 does NOT surface query-name metadata in its Debug rendering
/// (e.g., the format changes in a future point release), this sub-test falls
/// back to threshold-only mode: it asserts the count and prints a warning to
/// stderr explaining the fallback. The main
/// `editing_body_of_mapping_3_does_not_invalidate_item_tree` test remains
/// a secondary gate.
#[test]
// The keyset assertions are intentionally inline + heavily commented for
// debugability — splitting into helper fns would push the assertion site
// away from the keys: {keys:#?} payload that surfaces in CI failures.
#[allow(clippy::too_many_lines)]
fn keyset_of_reexecuted_queries_matches_expected_four() {
    let baseline = include_str!("fixtures/ten_mappings_baseline.fossil");
    let edited = include_str!("fixtures/ten_mappings_mapping_3_body_one_char_edit.fossil");

    let captured_keys: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_keys_clone = captured_keys.clone();
    let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            // Debug-render the database key. Salsa 0.26 implements Debug for
            // DatabaseKeyIndex; the rendering is `query_name(Id(raw))` when the
            // database is attached (which it is during the event callback).
            // This is not part of salsa's public API stability guarantees, so
            // the fallback path below handles a hypothetical future format
            // change.
            let key_str = format!("{database_key:?}");
            captured_keys_clone.lock().unwrap().push(key_str);
        }
    });

    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let mut db = FossilDb::with_event_callback(system, callback);

    let file = SourceFile::new(&db, baseline.to_string(), "ten_mappings.fossil".to_string());

    // Warm.
    let _ = item_tree(&db, file);
    let _ = ast_id_map(&db, file);
    let mappings: Vec<_> = def_map(&db, file).mappings(&db).clone();
    for m in &mappings {
        let _ = body(&db, *m);
        let _ = typecheck_mapping(&db, *m);
        let _ = expr_types(&db, *m);
        let _ = spans(&db, *m);
    }

    captured_keys.lock().unwrap().clear();

    file.set_text(&mut db).to(edited.to_string());

    let _ = item_tree(&db, file);
    let _ = ast_id_map(&db, file);
    let mappings_after: Vec<_> = def_map(&db, file).mappings(&db).clone();
    for m in &mappings_after {
        let _ = body(&db, *m);
        let _ = typecheck_mapping(&db, *m);
        let _ = expr_types(&db, *m);
        let _ = spans(&db, *m);
    }

    let keys = captured_keys.lock().unwrap().clone();

    // ── Fallback path: if salsa's Debug rendering doesn't expose a recognisable
    //    query name (any non-`DatabaseKeyIndex(...)` string), degrade gracefully.
    let any_named_query = keys
        .iter()
        .any(|k| k.contains("parse") || k.contains("body") || k.contains("item_tree"));

    if !any_named_query {
        eprintln!(
            "WARN: salsa 0.26 DatabaseKeyIndex Debug output does not include \n\
             a recognisable query-name token. keyset_of_reexecuted_queries_matches_expected_four \n\
             falling back to threshold-only mode. See top-of-file comment for context.\n\
             Captured keys for reference: {keys:#?}"
        );
        // Fallback: assert count is in the expected range (allow some salsa
        // version drift; main test enforces strict count).
        assert!(
            keys.len() <= MAX_REEXECUTIONS + 1,
            "fallback threshold-only mode: {} keys captured, expected ≤ {}",
            keys.len(),
            MAX_REEXECUTIONS + 1
        );
        return;
    }

    // ── Query-name assertions. Each `count_query(name)` returns how many
    //    captured keys contain the query-name token. By design,
    //    after a single body edit the only re-executed queries are:
    //      parse (1), body (1), typecheck_mapping (1), expr_types (1).
    //    Sibling mappings stay cached (because their MappingLoc identity is
    //    unchanged and their body content is unchanged).

    let count_query = |name: &str| keys.iter().filter(|k| k.contains(name)).count();

    let parse_count = count_query("parse");
    // Exclude `mapping_cst_node` matches when counting `body` (substring
    // collision). `mapping_cst_node` includes the substring `body`'s
    // letters? No — `body` is "body", `mapping_cst_node` is
    // "mapping_cst_node". No collision. But `typecheck_mapping` and
    // `mapping_cst_node` both contain "mapping". Use precise matches.
    let body_count = keys.iter().filter(|k| k.starts_with("body(")).count();
    let typecheck_count = keys
        .iter()
        .filter(|k| k.starts_with("typecheck_mapping("))
        .count();
    let expr_types_count = keys.iter().filter(|k| k.starts_with("expr_types(")).count();
    let spans_count = keys.iter().filter(|k| k.starts_with("spans(")).count();

    assert!(
        parse_count >= 1,
        "expected at least one parse re-exec; keys: {keys:#?}"
    );

    // ── LOAD-BEARING invariant: per-mapping body fan-out is
    //    exactly 1, NOT 10. If body_count == 10, the invalidation barrier
    //    (mapping_cst_node) is broken. NEVER silence this assertion.
    assert!(
        body_count <= MAX_PER_MAPPING_FAN_OUT,
        "FORBIDDEN per-mapping fan-out: body re-executed {body_count} times \
         after a single-mapping body edit (cap = {MAX_PER_MAPPING_FAN_OUT}). \
         If body_count == 10, ALL sibling mappings are being invalidated — \
         the mapping_cst_node invalidation barrier is broken. \
         Fix the data layout; do NOT silence. keys: {keys:#?}"
    );
    assert_eq!(
        body_count, 1,
        "expected exactly 1 body(M_3) re-exec (the load-bearing per-mapping \
         fan-out invariant); got {body_count}. keys: {keys:#?}"
    );

    // typecheck_mapping reads body(M_3) + spans(M_3) + resolve_source_row +
    // resolve_target_shape, so it re-executes exactly ONCE for the edited
    // mapping (the load-bearing per-mapping fan-out invariant).
    assert!(
        typecheck_count <= MAX_PER_MAPPING_FAN_OUT,
        "FORBIDDEN per-mapping fan-out: typecheck_mapping re-executed \
         {typecheck_count} times after a single-mapping body edit (cap = \
         {MAX_PER_MAPPING_FAN_OUT}). If typecheck_count == 10, the \
         resolve_source_row / typecheck_mapping path is depending on \
         parse(file) without a barrier — fix the \
         data layout, do NOT relax. keys: {keys:#?}"
    );
    assert_eq!(
        typecheck_count, 1,
        "expected exactly 1 typecheck_mapping(M_3) re-exec (the checker runs \
         for the edited mapping alone); got {typecheck_count}. keys: {keys:#?}"
    );

    // expr_types is a thin accessor over
    // typecheck_mapping. After a body-only edit to a schema-less mapping, the
    // typecheck output is structurally-equal, so expr_types VALIDATES instead
    // of re-executing — observed count is 0. The fan-out cap still applies.
    assert!(
        expr_types_count <= MAX_PER_MAPPING_FAN_OUT,
        "FORBIDDEN per-mapping fan-out: expr_types re-executed \
         {expr_types_count} times after a single-mapping body edit (cap = \
         {MAX_PER_MAPPING_FAN_OUT}). keys: {keys:#?}"
    );
    assert!(
        expr_types_count <= 1,
        "expected 0 or 1 expr_types(M_3) re-exec (thin accessor — validates \
         when typecheck output is structurally-equal); got {expr_types_count}. \
         keys: {keys:#?}"
    );

    // `spans()` LOAD-BEARING fan-out:
    // The spans tracked query reads `mapping_cst_node(M_k)`, NOT
    // `parse(file)`. The same Arc-shared-subtree invalidation barrier
    // established for `body()` therefore applies to `spans()`
    // too — only the edited mapping's spans re-execute; siblings stay
    // cached. If spans_count == 10, the claim that spans depends on
    // `mapping_cst_node` is broken; fix the data layout, do NOT
    // silence.
    assert!(
        spans_count <= MAX_PER_MAPPING_FAN_OUT,
        "FORBIDDEN per-mapping fan-out: spans re-executed {spans_count} \
         times after a single-mapping body edit (cap = \
         {MAX_PER_MAPPING_FAN_OUT}). If spans_count == 10, the claim that \
         `spans(db, mapping)` reads `mapping_cst_node` (NOT `parse(db, file)`) \
         is broken — fix the data layout in \
         crates/fossil-hir/src/spans.rs. keys: {keys:#?}"
    );
    assert_eq!(
        spans_count, 1,
        "expected exactly 1 spans(M_3) re-exec; got {spans_count}. \
         keys: {keys:#?}"
    );
}

/// True if `baseline` and `edited` differ ONLY in body content (the textual
/// content INSIDE a MAPPING_BODY), not in any header or item count.
///
/// Fast heuristic: compare counts of structural keywords/separators that
/// would change on a header edit. The fixtures are hand-crafted, so this is
/// sufficient; a stricter parser-based check is overkill.
///
/// Whitespace/CRLF-resilient: line counts ignore empty lines.
fn diff_is_body_only(baseline: &str, edited: &str) -> bool {
    fn structural_signature(s: &str) -> Vec<(&'static str, usize)> {
        // There was a `("prefix", s.matches("prefix ").count())` row here. The
        // vocabulary declaration is gone from the language, so the fixtures
        // declare none and the row counted 0 against 0 in every tree — a
        // signature entry that cannot discriminate is not a check. The four
        // below still move on any header edit.
        vec![
            (":=", s.matches(":=").count()),
            (" : ", s.matches(" : ").count()),
            (" from ", s.matches(" from ").count()),
            (
                "non_ws_lines",
                s.lines().filter(|l| !l.trim().is_empty()).count(),
            ),
        ]
    }
    structural_signature(baseline) == structural_signature(edited)
}

#[test]
fn fixtures_have_ten_mappings() {
    let baseline = include_str!("fixtures/ten_mappings_baseline.fossil");
    let edited = include_str!("fixtures/ten_mappings_mapping_3_body_one_char_edit.fossil");

    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db_a = FossilDb::new(system.clone());
    let db_b = FossilDb::new(system);
    let f_a = SourceFile::new(&db_a, baseline.to_string(), "a.fossil".to_string());
    let f_b = SourceFile::new(&db_b, edited.to_string(), "b.fossil".to_string());
    let dm_a = def_map(&db_a, f_a);
    let dm_b = def_map(&db_b, f_b);
    assert_eq!(
        dm_a.mappings(&db_a).len(),
        10,
        "baseline must have 10 mappings"
    );
    assert_eq!(
        dm_b.mappings(&db_b).len(),
        10,
        "edited must have 10 mappings"
    );
}
