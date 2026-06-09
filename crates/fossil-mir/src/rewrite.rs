//! The MIR rewriting engine — a **plain-Rust fixpoint** over the `Op` vec.
//!
//! [`rewrite`] applies the structural rules R1–R6 (`operator-algebra.md` §4.1,
//! Min Oo & Hartig) and the typed Fossil-specific rules R7–R10
//! (`operator-algebra.md` §4.2) to a [`MirGraph`] until no rule fires (a
//! fixpoint) or an iteration cap is hit. R7–R10 share the same `try_rN` rule
//! list and lean on the plain-Rust constant folder in [`crate::eval`]
//! (`partial_eval` / `static_truth`) so they add ZERO new tracked queries.
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

use smol_str::SmolStr;

use crate::eval::{partial_eval, static_truth};
use crate::graph::MirGraph;
use crate::op::{CmpOp, Expr, Op};
use crate::schema::{free_cols, schema_of};

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
            | try_r6(db, &mut ops)
            | try_r7(db, &mut ops)
            | try_r8(db, &mut ops)
            | try_r9(db, &mut ops)
            | try_r10(db, &mut ops);

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
            | Op::EmitVertex { input, .. }
            | Op::EmitEdge { input, .. }
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
fn insert_op<'db>(ops: &mut Vec<Op<'db>>, op: Op<'db>) -> usize {
    ops.push(op);
    ops.len() - 1
}

/// Redirect every op whose input/left/right is `from` to point at `to`,
/// EXCEPT the op at `except` (the rule's own freshly-rewired node, which must
/// keep its new wiring). Used by the node-count-changing rules (R3/R4/R5/R6)
/// to rewire the consumers of the old top node onto the new top node.
fn redirect_consumers(ops: &mut [Op<'_>], from: usize, to: usize, except: usize) {
    for (idx, op) in ops.iter_mut().enumerate() {
        if idx == except {
            continue;
        }
        match op {
            Op::Project { input, .. }
            | Op::Extend { input, .. }
            | Op::Rename { input, .. }
            | Op::Filter { input, .. }
            | Op::GroupBy { input, .. }
            | Op::Aggregate { input, .. }
            | Op::Distinct { input, .. }
            | Op::TripleEmit { input, .. }
            | Op::EmitVertex { input, .. }
            | Op::EmitEdge { input, .. }
            | Op::Sink { input, .. } => {
                if *input == from {
                    *input = to;
                }
            }
            Op::Join { left, right, .. } | Op::Union { left, right } => {
                if *left == from {
                    *left = to;
                }
                if *right == from {
                    *right = to;
                }
            }
            Op::Source { .. } | Op::Empty { .. } => {}
        }
    }
}

/// `true` iff `op` references `node` as one of its inputs.
///
/// The fan-out=1 MIR shape means a node has at most one consumer; rules that
/// swap node roles in place rely on this so a rewrite cannot strand a second
/// consumer with a stale view (see [`consumer_count`]).
const fn references(op: &Op<'_>, node: usize) -> bool {
    match op {
        Op::Project { input, .. }
        | Op::Extend { input, .. }
        | Op::Rename { input, .. }
        | Op::Filter { input, .. }
        | Op::GroupBy { input, .. }
        | Op::Aggregate { input, .. }
        | Op::Distinct { input, .. }
        | Op::TripleEmit { input, .. }
        | Op::EmitVertex { input, .. }
        | Op::EmitEdge { input, .. }
        | Op::Sink { input, .. } => *input == node,
        Op::Join { left, right, .. } | Op::Union { left, right } => *left == node || *right == node,
        Op::Source { .. } | Op::Empty { .. } => false,
    }
}

/// Count how many ops (other than `node` itself) reference `node` as an input.
fn consumer_count(ops: &[Op<'_>], node: usize) -> usize {
    ops.iter()
        .enumerate()
        .filter(|(idx, op)| *idx != node && references(op, node))
        .count()
}

// ---------------------------------------------------------------------------
// R1–R6 — structural rules (Min Oo & Hartig, operator-algebra.md §4.1).
//
// Each `try_rN` scans the op vec for its trigger pattern, applies the rewrite
// to the FIRST match (returning `true`), and otherwise returns `false`. The
// fixpoint loop re-runs the whole rule list, so applying to the first match per
// call is sufficient. Rules push in ONE canonical direction (R3/R5 push DOWN)
// so they cannot oscillate, and each rule's output no longer matches its own
// trigger (idempotency, SC#2).
// ---------------------------------------------------------------------------

/// R1 — filter fusion: `filter(filter(s, p), q)` → `filter(s, p ∧ q)`.
///
/// Fuses an outer `Filter` whose input is itself a `Filter` into one whose
/// pred is `BinOp(And, p, q)` over the inner filter's input `s`. The inner
/// `Filter` op is removed and indices renumbered. Idempotent: one `Filter`
/// remains; no nested-filter pattern survives.
fn try_r1<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    for outer in 0..ops.len() {
        let Op::Filter {
            input: inner,
            pred: q,
        } = &ops[outer]
        else {
            continue;
        };
        let inner = *inner;
        let q = q.clone();
        let Some(Op::Filter { input: s, pred: p }) = ops.get(inner) else {
            continue;
        };
        // Only fuse when the inner filter feeds the outer exclusively (the
        // fan-out=1 MIR shape guarantees this; guard defensively).
        if consumer_count(ops, inner) != 1 {
            continue;
        }
        let s = *s;
        let p = p.clone();
        let fused = Expr::BinOp {
            op: CmpOp::And,
            lhs: Box::new(p),
            rhs: Box::new(q),
            ty: bool_ty(db),
        };
        ops[outer] = Op::Filter {
            input: s,
            pred: fused,
        };
        // Remove the now-orphaned inner filter; consumers of `inner` (only the
        // outer, already rewired to `s`) are redirected to `s`.
        reindex_after_removal(ops, inner, s);
        return true;
    }
    false
}

/// R2 — project fusion: `project(project(s, c₁), c₂)` → `project(s, c₁ ∩ c₂)`.
///
/// Intersects the column sets, preserving the OUTER (`c₂`) order. The inner
/// `Project` is removed. Idempotent: one `Project` remains.
fn try_r2<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    let _ = db;
    for outer in 0..ops.len() {
        let Op::Project {
            input: inner,
            cols: c2,
        } = &ops[outer]
        else {
            continue;
        };
        let inner = *inner;
        let c2 = c2.clone();
        let Some(Op::Project { input: s, cols: c1 }) = ops.get(inner) else {
            continue;
        };
        if consumer_count(ops, inner) != 1 {
            continue;
        }
        let s = *s;
        let c1 = c1.clone();
        // c₁ ∩ c₂, preserving c₂ order (the outer projection wins on order).
        let intersected: Vec<SmolStr> = c2.iter().filter(|c| c1.contains(c)).cloned().collect();
        ops[outer] = Op::Project {
            input: s,
            cols: intersected,
        };
        reindex_after_removal(ops, inner, s);
        return true;
    }
    false
}

/// R3 — project-below-filter pushdown:
/// `project(filter(s, p), c)` → `filter(project(s, c), p)` **if `free(p) ⊆ c`**.
///
/// Pushes the projection DOWN below the filter (canonical direction → cannot
/// oscillate). The guard `free(p) ⊆ c` ensures the predicate's columns survive
/// the projection. After the swap the top node is the `Filter`, so consumers of
/// the old top `Project` are redirected onto it. Idempotent: the canonical form
/// (`filter` above `project`) never re-triggers R3 (R3 only fires on
/// `project(filter(...))`).
fn try_r3<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    for proj in 0..ops.len() {
        let Op::Project {
            input: filt,
            cols: c,
        } = &ops[proj]
        else {
            continue;
        };
        let filt = *filt;
        let c = c.clone();
        let Some(Op::Filter { input: s, pred: p }) = ops.get(filt) else {
            continue;
        };
        if consumer_count(ops, filt) != 1 {
            continue;
        }
        let s = *s;
        let p = p.clone();
        // Guard: free(p) ⊆ c.
        let free = free_cols(&p);
        let c_set: std::collections::BTreeSet<SmolStr> = c.iter().cloned().collect();
        if !free.is_subset(&c_set) {
            continue;
        }
        // Rewrite IN PLACE swapping roles:
        //   ops[filt]  becomes the Project(s, c)   (was Filter)
        //   ops[proj]  becomes the Filter(filt, p) (was Project)
        ops[filt] = Op::Project { input: s, cols: c };
        ops[proj] = Op::Filter {
            input: filt,
            pred: p,
        };
        // `proj` was the top node; it stays the top (now a Filter) and still
        // occupies index `proj`, so external consumers need no redirect.
        let _ = db;
        return true;
    }
    false
}

/// R4 — filter-over-union distribution:
/// `filter(union(s₁, s₂), p)` → `union(filter(s₁, p), filter(s₂, p))`.
///
/// Pushes the filter into BOTH union arms. Two new `Filter` ops are appended;
/// the `Union`'s inputs are rewired to them; the old top `Filter` is replaced
/// by the `Union` and its consumers redirected. Idempotent: the outer
/// filter-over-union pattern is gone.
fn try_r4<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    let _ = db;
    for filt in 0..ops.len() {
        let Op::Filter { input: un, pred: p } = &ops[filt] else {
            continue;
        };
        let un = *un;
        let p = p.clone();
        let Some(Op::Union {
            left: s1,
            right: s2,
        }) = ops.get(un)
        else {
            continue;
        };
        if consumer_count(ops, un) != 1 {
            continue;
        }
        let (s1, s2) = (*s1, *s2);
        // Append filter(s₁, p) and filter(s₂, p).
        let f1 = insert_op(
            ops,
            Op::Filter {
                input: s1,
                pred: p.clone(),
            },
        );
        let f2 = insert_op(ops, Op::Filter { input: s2, pred: p });
        // Rewire the Union onto the two new filters.
        ops[un] = Op::Union {
            left: f1,
            right: f2,
        };
        // The old top Filter becomes a passthrough to the Union; redirect its
        // consumers onto the Union and drop the now-dead Filter node.
        redirect_consumers(ops, filt, un, filt);
        reindex_after_removal(ops, filt, un);
        return true;
    }
    false
}

/// R5 — filter-into-join pushdown:
/// `filter(join(s₁, s₂, c), p)` → `join(filter(s₁, p), s₂, c)`
/// **if `free(p) ⊆ schema(s₁)`**.
///
/// Pushes the predicate into the LEFT join input only (canonical direction).
/// The guard ensures the predicate's columns are available in `s₁`; once
/// pushed, the guard is false at the join's top so R5 cannot re-fire.
fn try_r5<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    for filt in 0..ops.len() {
        let Op::Filter { input: jn, pred: p } = &ops[filt] else {
            continue;
        };
        let jn = *jn;
        let p = p.clone();
        let Some(Op::Join {
            left,
            right,
            on,
            kind,
            left_name,
            right_name,
        }) = ops.get(jn)
        else {
            continue;
        };
        if consumer_count(ops, jn) != 1 {
            continue;
        }
        let (left, right) = (*left, *right);
        let (on, kind) = (on.clone(), *kind);
        let (left_name, right_name) = (left_name.clone(), right_name.clone());
        // Guard: free(p) ⊆ schema(s₁) (the LEFT input schema).
        let free = free_cols(&p);
        let left_schema: std::collections::BTreeSet<SmolStr> =
            schema_of(db, ops, left).into_iter().collect();
        if !free.is_subset(&left_schema) {
            continue;
        }
        // Append filter(s₁, p), rewire the join's left onto it.
        let f1 = insert_op(
            ops,
            Op::Filter {
                input: left,
                pred: p,
            },
        );
        ops[jn] = Op::Join {
            left: f1,
            right,
            on,
            kind,
            left_name,
            right_name,
        };
        // The old top Filter becomes a passthrough to the Join; redirect its
        // consumers and drop the dead Filter.
        redirect_consumers(ops, filt, jn, filt);
        reindex_after_removal(ops, filt, jn);
        return true;
    }
    false
}

/// R6 — rename expansion:
/// `rename(s, a, b)` → `extend(project(s, schema(s) \ {a}), b, ref(a))`.
///
/// Replaces a `Rename` with an `Extend` over a `Project` (per
/// operator-algebra.md §4.1). `schema(s) \ {a}` drops the renamed-away column;
/// the new `Extend` re-introduces it under the new name `b` as a `ColRef(a)`.
/// Idempotent: no `Rename` remains to re-trigger.
fn try_r6<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    for rn in 0..ops.len() {
        let Op::Rename {
            input: s,
            old: a,
            new: b,
        } = &ops[rn]
        else {
            continue;
        };
        let s = *s;
        let a = a.clone();
        let b = b.clone();
        // schema(s) \ {a}.
        let cols: Vec<SmolStr> = schema_of(db, ops, s)
            .into_iter()
            .filter(|c| *c != a)
            .collect();
        // Append project(s, schema(s) \ {a}), then rewrite the Rename node into
        // an Extend over that project. The Extend keeps index `rn`, so external
        // consumers need no redirect.
        let proj = insert_op(ops, Op::Project { input: s, cols });
        ops[rn] = Op::Extend {
            input: proj,
            field: b,
            expr: Expr::ColRef {
                source: SmolStr::default(),
                column: a,
            },
        };
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// R7–R10 — Fossil-specific typed rules (operator-algebra.md §4.2).
//
// These lean on the plain-Rust constant folder ([`crate::eval`]): R7 folds an
// `Extend` expression in place; R8 / R9 ask `static_truth` whether a `Filter`
// predicate is statically decidable; R10 fuses nested `GroupBy` keys. Like
// R1–R6 they fire on the FIRST match per call and are idempotent (their output
// no longer matches their own trigger).
// ---------------------------------------------------------------------------

/// R7 — partial-eval an `Extend` expression:
/// `extend(s, f, e)` → `extend(s, f, partial_eval(e))` when folding *changed*
/// the expression.
///
/// Fires only when `partial_eval(e) != e` (otherwise nothing to do — this is
/// the firing guard that prevents infinite re-fire). Idempotent because
/// `partial_eval` is a normal form (`crate::eval`): the folded expression is
/// stable, so a second pass leaves it alone and R7 does not re-trigger.
fn try_r7<'db>(db: &'db dyn fossil_base::Db, ops: &mut [Op<'db>]) -> bool {
    let _ = db;
    for op in ops.iter_mut() {
        let Op::Extend { expr, .. } = op else {
            continue;
        };
        let folded = partial_eval(expr);
        if &folded == expr {
            continue; // nothing to fold — leave untouched (idempotency guard)
        }
        *expr = folded;
        return true;
    }
    false
}

/// R8 — drop a statically-true `Filter`:
/// `filter(s, p)` where `static_truth(p) == Some(true)` → remove the `Filter`,
/// rewiring its consumers onto its input `s`.
///
/// Idempotent: the `Filter` is gone, so there is no statically-true predicate
/// left to re-trigger.
fn try_r8<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    let _ = db;
    for filt in 0..ops.len() {
        let Op::Filter { input: s, pred } = &ops[filt] else {
            continue;
        };
        if static_truth(pred) != Some(true) {
            continue;
        }
        let s = *s;
        // Rewire consumers of the filter onto `s`, then drop the filter node.
        // `reindex_after_removal` remaps indices pointing AT `filt` to `s` and
        // shifts the tail down, so the consumer rewiring is implicit.
        reindex_after_removal(ops, filt, s);
        return true;
    }
    false
}

/// R9 — replace a statically-false `Filter` with `Op::Empty`:
/// `filter(s, p)` where `static_truth(p) == Some(false)` →
/// `Op::Empty { schema: schema(s) }`.
///
/// The `Empty` node carries the schema the filtered relation would have had so
/// codegen (plan 04-04) emits a `SELECT … WHERE false` shell of the right
/// shape (ADR-0011). Replaced IN PLACE (index `filt` unchanged), so consumers
/// need no rewiring. Idempotent: `Empty` has no predicate, so R9 cannot
/// re-fire.
fn try_r9<'db>(db: &'db dyn fossil_base::Db, ops: &mut [Op<'db>]) -> bool {
    for filt in 0..ops.len() {
        let Op::Filter { input: s, pred } = &ops[filt] else {
            continue;
        };
        if static_truth(pred) != Some(false) {
            continue;
        }
        let s = *s;
        let schema = schema_of(db, ops, s);
        ops[filt] = Op::Empty { schema };
        return true;
    }
    false
}

/// R10 — group-by fusion:
/// `group_by(group_by(s, k₁), k₂)` → `group_by(s, k₁ ∪ k₂)` when
/// `k₁ ⊆ schema(s)` and the aggregations compose.
///
/// For v0.1 we fuse the key sets (union, preserving k₁ order then the new
/// keys from k₂). We DO NOT fire when an `Aggregate` sits between the two
/// `GroupBy`s — an intervening aggregation breaks key composition. The inner
/// `GroupBy` is removed and indices renumbered. Idempotent: a single `GroupBy`
/// remains, so the nested pattern is gone.
fn try_r10<'db>(db: &'db dyn fossil_base::Db, ops: &mut Vec<Op<'db>>) -> bool {
    for outer in 0..ops.len() {
        let Op::GroupBy {
            input: inner,
            keys: k2,
        } = &ops[outer]
        else {
            continue;
        };
        let inner = *inner;
        let k2 = k2.clone();
        let Some(Op::GroupBy { input: s, keys: k1 }) = ops.get(inner) else {
            continue;
        };
        // Only fuse when the inner group-by feeds the outer exclusively (no
        // intervening Aggregate could sit on this edge under fan-out=1, but
        // guard defensively).
        if consumer_count(ops, inner) != 1 {
            continue;
        }
        let s = *s;
        let k1 = k1.clone();
        // Guard: k₁ ⊆ schema(s) (the keys the inner group-by grouped by must be
        // columns of its own input — i.e. the composition is well-formed).
        let s_schema: std::collections::BTreeSet<SmolStr> =
            schema_of(db, ops, s).into_iter().collect();
        if !k1.iter().all(|k| s_schema.contains(k)) {
            continue;
        }
        // k₁ ∪ k₂, preserving k₁ order then appending the new k₂ keys.
        let mut fused = k1;
        for k in &k2 {
            if !fused.contains(k) {
                fused.push(k.clone());
            }
        }
        ops[outer] = Op::GroupBy {
            input: s,
            keys: fused,
        };
        reindex_after_removal(ops, inner, s);
        return true;
    }
    false
}

/// The `bool` primitive type, for the fused `BinOp(And, …)` predicate in R1.
fn bool_ty(db: &dyn fossil_base::Db) -> fossil_hir::Ty<'_> {
    fossil_hir::Ty::new(
        db,
        fossil_hir::TyKind::Primitive(fossil_hir::Primitive::Bool),
    )
}
