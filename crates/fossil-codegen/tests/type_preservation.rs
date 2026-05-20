//! SC#3 — lowering preserves type info: `codegen(g) == codegen(erase_types(g))`.
//!
//! Mechanizes the conservative-extension claim (ADR-0013). For every one of the
//! 30 corpus mappings (shared via `include!("support/corpus_data.rs")`), this
//! suite codegens the typed graph `g` and the type-erased projection
//! `erase_types(g)` and asserts the generated SQL is byte-for-byte identical.
//!
//! If any codegen path ever began consuming a `Ty` for operational semantics,
//! the erased projection's sentinel type would surface a difference and this
//! test would fail — so the test is the executable witness that types are
//! erasable at codegen (operator-algebra.md §1/§9; type-system.md).
//!
//! The deterministic corpus loop is the SC#3-required property check; it is
//! snapshot-stable and covers every operator + every R1–R10 outcome.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::elidable_lifetime_names, dead_code)]

use std::sync::Arc;

use fossil_codegen::codegen_graph;
use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::erase_types;
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{AggFn, AggSpec, CmpOp, Expr, JoinKind, Op, SinkRef, SourceFormat};
use fossil_mir::rewrite;
use smol_str::SmolStr;

include!("support/corpus_data.rs");

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
    fossil_base::FossilDb::new(system)
}

/// Indexes into [`CORPUS`].
#[salsa::input]
struct Case {
    idx: usize,
}

/// Codegen both the typed corpus graph and its type-erased projection, returning
/// `(typed_sql, erased_sql)` — built inside a tracked frame (`MirGraph::new` /
/// `erase_types` intern + create tracked structs, so they need a tracked frame).
#[salsa::tracked]
fn typed_and_erased_sql(db: &dyn fossil_base::Db, case: Case) -> (String, String) {
    let entry = &CORPUS[case.idx(db)];
    let ops = corpus_entry_ops(db, entry);
    let g = MirGraph::new(db, ops);
    let typed = codegen_graph(db, g).sql(db).clone();
    let erased = codegen_graph(db, erase_types(db, g)).sql(db).clone();
    (typed, erased)
}

/// SC#3 property: for every corpus mapping, the type-erased projection codegens
/// to byte-identical SQL as the typed graph (types carry no operational
/// meaning — ADR-0013).
#[test]
fn codegen_is_type_erasable_over_the_corpus() {
    let db = db();
    for (idx, entry) in CORPUS.iter().enumerate() {
        let case = Case::new(&db, idx);
        let (typed, erased) = typed_and_erased_sql(&db, case);
        // Guard against a degenerate pass (both empty): the SQL must be real.
        assert!(
            !typed.is_empty(),
            "generated SQL for {} must be non-empty",
            entry.name
        );
        assert_eq!(
            typed, erased,
            "SC#3 violated: codegen(g) != codegen(erase_types(g)) for {}\n\
             typed:\n{typed}\n\nerased:\n{erased}",
            entry.name
        );
    }
}
