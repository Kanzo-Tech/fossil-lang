//! SC#1 — the 30-mapping golden corpus → stable `DuckDB` SQL snapshots.
//!
//! Drives the shared [`CORPUS`] (defined in `corpus_data.rs`, `include!`d below)
//! through the [`fossil_codegen::codegen_graph`] seam and `insta`-snapshots the
//! generated SQL for every entry. CI fails on any SQL diff, locking:
//! - all 11 operators + `Op::Empty` (single-input + multi-input + aggregating),
//! - each R1–R10 rewrite OUTCOME (the corpus carries the post-`rewrite` graph),
//! - multi-`TripleEmit` mappings + the SC#4 named runtime assertion.
//!
//! The corpus is the SC#1 SNAPSHOT tier. The NATIVE-EXECUTION tier lives in
//! `fossil-runtime/tests/corpus_exec.rs` (which also writes the cross-engine
//! `native_baseline.json`); the SC#3 erase-types property tier lives in
//! `tests/type_preservation.rs` (also `include!`s `corpus_data.rs`).
//!
//! # The `#[salsa::tracked]` test seam
//!
//! `MirGraph::new` / `Ty` interning may only run inside a tracked frame, so the
//! per-entry graph is built inside [`codegen_case`], a tracked function keyed by
//! a [`Case`] index into [`CORPUS`].

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::elidable_lifetime_names, dead_code)]

use std::sync::Arc;

use fossil_codegen::codegen_graph;
use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{AggFn, AggSpec, CmpOp, Expr, JoinKind, Op, SinkRef, SourceFormat};
use fossil_mir::rewrite;
use smol_str::SmolStr;

include!("support/corpus_data.rs");

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
    fossil_base::FossilDb::new(system)
}

/// Indexes into [`CORPUS`].
#[salsa::input]
struct Case {
    idx: usize,
}

/// Build the chosen corpus entry's `MirGraph` (applying `rewrite` for the
/// `RewriteOutcome` entries) and codegen it — all inside a tracked frame.
#[salsa::tracked]
fn codegen_case<'db>(db: &'db dyn fossil_base::Db, case: Case) -> fossil_codegen::SqlPlan<'db> {
    let entry = &CORPUS[case.idx(db)];
    let ops = corpus_entry_ops(db, entry);
    codegen_graph(db, MirGraph::new(db, ops))
}

fn sql_for(idx: usize) -> String {
    let db = db();
    let case = Case::new(&db, idx);
    codegen_case(&db, case).sql(&db).clone()
}

/// Snapshot every one of the 30 corpus entries by NAME (stable snapshot files
/// `snapshots/corpus__<name>.snap`).
#[test]
fn corpus_sql_snapshots() {
    insta::with_settings!({ snapshot_path => "snapshots", prepend_module_to_snapshot => false }, {
        for (idx, entry) in CORPUS.iter().enumerate() {
            insta::assert_snapshot!(entry.name, sql_for(idx));
        }
    });
}

/// The corpus is exactly 30 mappings (SC#1 wording).
#[test]
fn corpus_is_thirty_mappings() {
    assert_eq!(CORPUS.len(), 30, "SC#1 mandates a 30-mapping corpus");
}

/// No `TyKind::Unknown` / `InferenceId` debug text may leak into ANY generated
/// SQL (invariant #8 + RESEARCH Pitfall 5): codegen ignores types, so no type
/// annotation should ever surface in the SQL string.
#[test]
fn no_type_text_leaks_into_corpus_sql() {
    for (idx, entry) in CORPUS.iter().enumerate() {
        let sql = sql_for(idx);
        assert!(
            !sql.contains("Unknown("),
            "TyKind::Unknown leaked into SQL for {}:\n{sql}",
            entry.name
        );
        assert!(
            !sql.contains("InferenceId"),
            "InferenceId leaked into SQL for {}:\n{sql}",
            entry.name
        );
        assert!(
            !sql.contains("todo!") && !sql.contains("unimplemented!"),
            "stub marker leaked into SQL for {}:\n{sql}",
            entry.name
        );
    }
}
