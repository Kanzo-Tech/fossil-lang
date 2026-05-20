//! The MIR rewriting engine — a **plain-Rust fixpoint** over the `Op` vec.
//!
//! [`rewrite`] applies the structural rules R1–R6 (`operator-algebra.md` §4.1,
//! Min Oo & Hartig) to a [`MirGraph`] until no rule fires (a fixpoint) or an
//! iteration cap is hit. R7–R10 (the typed Fossil-specific rules) land in plan
//! 04-03 and slot into the same `try_rN` rule list.
//!
//! # Why plain-Rust and NOT `#[salsa::tracked]` (ADR-0010)
//!
//! Each rewrite rule as its own tracked query would multiply the per-mapping
//! tracked-query count and threaten the `MAX_PER_MAPPING_FAN_OUT = 1`
//! invalidation barrier (ADR-0005 + plan 02-07). So [`rewrite`] is a plain
//! function. It is invoked ONCE from the tracked [`crate::lower::lower_to_mir`]
//! frame, so it is memoised with that query and adds ZERO new tracked queries.
//! `Box<Expr>` recursion inside the rules is *data*, not a trait object, so the
//! "no `Box<dyn Trait>` in Salsa queries" hard rule is upheld.
//!
//! # Termination (RESEARCH Pitfall 2 — oscillating rules)
//!
//! Three guarantees prevent non-termination:
//! 1. **Canonical single-direction push.** R3 / R5 push project / filter DOWN
//!    toward sources only; there is no inverse rule, so they cannot ping-pong.
//! 2. **Idempotent rules.** Each rule's trigger pattern no longer matches its
//!    own output (R1 leaves one Filter; R6 leaves no Rename; the R3/R5 guards
//!    are designed so the pushed form is canonical).
//! 3. **Iteration cap.** [`MAX_ITERS`] bounds the fixpoint loop; on overrun the
//!    engine calls [`fossil_base::delay_span_bug`] and returns the current vec
//!    (degrade, never hang).
//!
//! # Index discipline
//!
//! `Op` inputs (`input` / `left` / `right`) are `usize` indices into the op
//! vec, in topological order. Whenever a rule deletes or inserts an op it must
//! keep every index consistent. Rules replace an op in place (indices
//! unchanged) and use [`insert_op`] (append a new op and hand back its index)
//! so renumbering is localised; deletions go through [`reindex_after_removal`].

use crate::graph::MirGraph;
use crate::op::Op;

/// Fixpoint iteration cap. A well-formed R1–R6 pass converges in far fewer
/// passes than this (each rule strictly reduces a structural measure); the cap
/// is a termination backstop, not an expected limit.
const MAX_ITERS: usize = 100;

/// Apply the structural rewriting rules R1–R6 to `graph` until a fixpoint.
///
/// PLAIN RUST — NOT a tracked query (ADR-0010). Returns a fresh [`MirGraph`]
/// with the rewritten op vec. hello.fossil's MIR (`Source → Extend →
/// TripleEmit → Sink`) matches none of the R1–R6 triggers, so it is returned
/// unchanged and its SQL stays byte-identical.
#[must_use]
pub fn rewrite<'db>(db: &'db dyn fossil_base::Db, graph: MirGraph<'db>) -> MirGraph<'db> {
    let mut ops: Vec<Op<'db>> = graph.ops(db).clone();

    let mut iters = 0usize;
    loop {
        // One full pass: try every rule in order. `changed` records whether
        // *any* rule fired this pass.
        let changed = try_r1(db, &mut ops)
            | try_r2(db, &mut ops)
            | try_r3(db, &mut ops)
            | try_r4(db, &mut ops)
            | try_r5(db, &mut ops)
            | try_r6(db, &mut ops);

        if !changed {
            break; // fixpoint reached
        }

        iters += 1;
        if iters >= MAX_ITERS {
            // Degrade, never hang (RESEARCH Pitfall 2). The taint is dropped
            // intentionally — a non-converging optimiser must not abort the
            // compile; we return the best-effort vec and surface the bug.
            let _ = fossil_base::delay_span_bug(
                db,
                fossil_base::Span::new(0, 0),
                format!(
                    "MIR rewriting did not converge within {MAX_ITERS} iterations; \
                     returning the partially-rewritten graph (a rewrite rule is \
                     likely non-idempotent or oscillating)"
                ),
            );
            break;
        }
    }

    MirGraph::new(db, ops)
}

// ---------------------------------------------------------------------------
// Index-discipline helpers
// ---------------------------------------------------------------------------

/// Rewrite every `input` / `left` / `right` index in `ops` with `f`, skipping
/// indices for which `f` returns `None` (used by callers that delete an op and
/// shift the tail down). `Source` / `Empty` have no inputs and are untouched.
#[allow(dead_code)] // wired by R1–R6 in Task 2
fn map_indices(ops: &mut [Op<'_>], f: impl Fn(usize) -> usize) {
    for op in ops.iter_mut() {
        match op {
            Op::Project { input, .. }
            | Op::Extend { input, .. }
            | Op::Rename { input, .. }
            | Op::Filter { input, .. }
            | Op::GroupBy { input, .. }
            | Op::Aggregate { input, .. }
            | Op::Distinct { input, .. }
            | Op::TripleEmit { input, .. }
            | Op::Sink { input, .. } => *input = f(*input),
            Op::Join { left, right, .. } | Op::Union { left, right } => {
                *left = f(*left);
                *right = f(*right);
            }
            Op::Source { .. } | Op::Empty { .. } => {}
        }
    }
}

/// Remove the op at `removed` and shift every index that pointed past it down
/// by one (indices that pointed *at* `removed` are remapped to `redirect_to`,
/// i.e. the op that now occupies that slot in the data-flow).
///
/// This is the deletion primitive R1 (drop the inner Filter) builds on.
#[allow(dead_code)] // wired by R1–R6 in Task 2
fn reindex_after_removal(ops: &mut Vec<Op<'_>>, removed: usize, redirect_to: usize) {
    ops.remove(removed);
    // After the `remove`, every original index `i` maps to:
    //   i  < removed         → i           (unchanged)
    //   i == removed         → redirect_to' (its consumers are rewired)
    //   i  > removed         → i - 1        (shifted down)
    // We must also shift `redirect_to` itself if it sat past `removed`.
    let redirect_shifted = if redirect_to > removed {
        redirect_to - 1
    } else {
        redirect_to
    };
    map_indices(ops, |i| match i.cmp(&removed) {
        std::cmp::Ordering::Equal => redirect_shifted,
        std::cmp::Ordering::Greater => i - 1,
        std::cmp::Ordering::Less => i,
    });
}

/// Append `op` to the vec and return its index. New ops always go at the end so
/// existing indices stay valid; topo order is preserved because a newly
/// inserted op only references ops already present (lower indices).
#[allow(dead_code)] // wired by R1–R6 in Task 2
fn insert_op<'db>(ops: &mut Vec<Op<'db>>, op: Op<'db>) -> usize {
    ops.push(op);
    ops.len() - 1
}

// ---------------------------------------------------------------------------
// R1–R6 — structural rules (Min Oo & Hartig, operator-algebra.md §4.1).
// Implemented in Task 2.
// ---------------------------------------------------------------------------

// Transitional: the rule bodies are filled in Task 2. The stubs neither read
// nor mutate their args, which trips several clippy lints that vanish once the
// real bodies land — suppress them on the stub block only.
#[allow(
    unused_variables,
    clippy::needless_pass_by_ref_mut,
    clippy::ptr_arg,
    dead_code
)]
fn try_r1<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    false
}

#[allow(
    unused_variables,
    clippy::needless_pass_by_ref_mut,
    clippy::ptr_arg,
    dead_code
)]
fn try_r2<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    false
}

#[allow(
    unused_variables,
    clippy::needless_pass_by_ref_mut,
    clippy::ptr_arg,
    dead_code
)]
fn try_r3<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    false
}

#[allow(
    unused_variables,
    clippy::needless_pass_by_ref_mut,
    clippy::ptr_arg,
    dead_code
)]
fn try_r4<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    false
}

#[allow(
    unused_variables,
    clippy::needless_pass_by_ref_mut,
    clippy::ptr_arg,
    dead_code
)]
fn try_r5<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    false
}

#[allow(
    unused_variables,
    clippy::needless_pass_by_ref_mut,
    clippy::ptr_arg,
    dead_code
)]
fn try_r6<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    false
}
