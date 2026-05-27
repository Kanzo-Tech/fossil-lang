//! Static evaluation pass — constant-folding over the typed [`Expr`].
//!
//! [`partial_eval`] is a plain-Rust bottom-up constant folder: it collapses
//! `Concat` of literals, `BinOp` of literals, and `And` / `Or` with a constant
//! operand. It is the analysis that drives the typed rewrite rules R7–R10
//! (`crate::rewrite`): R7 folds an `Extend`'s expression in place; R8 / R9 ask
//! [`static_truth`] whether a `Filter` predicate is statically decidable.
//!
//! # Why plain Rust (no Salsa, no runtime data)
//!
//! Folding is a pure structural recursion over the `Expr` ADT — there is no
//! query input to track and no row data to read (the whole point is *static*
//! evaluation). Keeping it a plain function means R7–R10 add ZERO new tracked
//! queries when they call it from the (already plain-Rust) rewrite fixpoint,
//! preserving `MAX_PER_MAPPING_FAN_OUT = 1` (ADR-0010).
//!
//! # Normal-form / idempotency property
//!
//! `partial_eval` is a *normal form*: `partial_eval(partial_eval(e)) ==
//! partial_eval(e)`. The fold recurses into children first (bottom-up), so a
//! single pass reaches the fixpoint — re-folding a folded expression changes
//! nothing. This is what gives R7 its idempotency for free (R7 only fires when
//! folding *changed* the expression, and the folded form is stable).
//!
//! Data-dependent leaves (`ColRef`, `Call`) and the `Assert` wrapper are left
//! untouched (their children are still folded). `Assert` is a no-op wrapper
//! until plan 04-06 threads the runtime assertion; folding through it would
//! drop the assertion name, so we preserve the node and fold only `inner`.

use crate::op::{CmpOp, Expr};

/// Constant-fold `expr` bottom-up, returning a (possibly) simplified `Expr`.
///
/// Folds performed:
/// - `Concat(LitString(a), LitString(b))` → `LitString(a ++ b)`
/// - `BinOp(And, LitBool(true), x)` → `x`; `BinOp(And, LitBool(false), _)` →
///   `LitBool(false)` (and the symmetric right-operand cases)
/// - `BinOp(Or, LitBool(true), _)` → `LitBool(true)`;
///   `BinOp(Or, LitBool(false), x)` → `x` (and the symmetric cases)
/// - `BinOp(cmp, LitString(a), LitString(b))` → `LitBool(a cmp b)` for the
///   ordered/equality comparators (`Eq`/`Ne`/`Lt`/`Le`/`Gt`/`Ge`)
/// - `BinOp(cmp, LitBool(a), LitBool(b))` → `LitBool(a cmp b)` for `Eq`/`Ne`
///
/// `ColRef` / `Call` are data-dependent and never fold to a constant (their
/// arguments are still folded recursively). `Assert` keeps its wrapper and
/// folds only the inner expression.
///
/// Idempotent (a normal form): `partial_eval(partial_eval(e)) ==
/// partial_eval(e)`.
#[must_use]
pub fn partial_eval<'db>(expr: &Expr<'db>) -> Expr<'db> {
    match expr {
        // Leaves that are already constant or data-dependent.
        Expr::LitString(_) | Expr::LitBool(_) | Expr::ColRef { .. } => expr.clone(),

        // Concat: fold children, then collapse if both are string literals.
        Expr::Concat(lhs, rhs) => {
            let lhs = partial_eval(lhs);
            let rhs = partial_eval(rhs);
            if let (Expr::LitString(a), Expr::LitString(b)) = (&lhs, &rhs) {
                let mut joined = String::with_capacity(a.len() + b.len());
                joined.push_str(a);
                joined.push_str(b);
                return Expr::LitString(joined.into());
            }
            Expr::Concat(Box::new(lhs), Box::new(rhs))
        }

        // Call: data-dependent (the stdlib → SQL mapping lands in Phase 5).
        // Fold the arguments but never collapse the call itself.
        Expr::Call { func, args, ty } => Expr::Call {
            func: func.clone(),
            args: args.iter().map(partial_eval).collect(),
            ty: *ty,
        },

        // BinOp: fold children, then apply the constant-folding algebra.
        Expr::BinOp { op, lhs, rhs, ty } => {
            let lhs = partial_eval(lhs);
            let rhs = partial_eval(rhs);
            fold_binop(*op, lhs, rhs, *ty)
        }

        // Assert: a no-op wrapper until plan 04-06. Preserve the name/span and
        // fold only the inner expression (folding it away would drop the
        // assertion).
        Expr::Assert {
            name,
            span_line,
            inner,
        } => Expr::Assert {
            name: name.clone(),
            span_line: *span_line,
            inner: Box::new(partial_eval(inner)),
        },
    }
}

/// Apply the constant-folding algebra to a `BinOp` whose operands are already
/// folded. Returns the folded result, or the rebuilt `BinOp` when no rule
/// applies.
fn fold_binop<'db>(
    op: CmpOp,
    lhs: Expr<'db>,
    rhs: Expr<'db>,
    ty: fossil_hir::Ty<'db>,
) -> Expr<'db> {
    match op {
        // Short-circuit boolean algebra (handles a constant on either side).
        CmpOp::And => match (&lhs, &rhs) {
            (Expr::LitBool(false), _) | (_, Expr::LitBool(false)) => Expr::LitBool(false),
            (Expr::LitBool(true), _) => rhs,
            (_, Expr::LitBool(true)) => lhs,
            _ => rebuild(op, lhs, rhs, ty),
        },
        CmpOp::Or => match (&lhs, &rhs) {
            (Expr::LitBool(true), _) | (_, Expr::LitBool(true)) => Expr::LitBool(true),
            (Expr::LitBool(false), _) => rhs,
            (_, Expr::LitBool(false)) => lhs,
            _ => rebuild(op, lhs, rhs, ty),
        },
        // Comparators over two literals of the same kind.
        CmpOp::Eq | CmpOp::Ne | CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
            match (&lhs, &rhs) {
                (Expr::LitString(a), Expr::LitString(b)) => Expr::LitBool(cmp_str(op, a, b)),
                (Expr::LitBool(a), Expr::LitBool(b)) => match op {
                    CmpOp::Eq => Expr::LitBool(a == b),
                    CmpOp::Ne => Expr::LitBool(a != b),
                    // Ordering over booleans is not meaningful — leave unfolded.
                    _ => rebuild(op, lhs, rhs, ty),
                },
                _ => rebuild(op, lhs, rhs, ty),
            }
        }
    }
}

/// Compare two string literals under an ordering/equality comparator.
fn cmp_str(op: CmpOp, a: &str, b: &str) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
        // `And`/`Or` never reach `cmp_str`.
        CmpOp::And | CmpOp::Or => unreachable!("cmp_str called with a boolean connective"),
    }
}

/// Rebuild a `BinOp` from already-folded operands (no rule fired).
fn rebuild<'db>(op: CmpOp, lhs: Expr<'db>, rhs: Expr<'db>, ty: fossil_hir::Ty<'db>) -> Expr<'db> {
    Expr::BinOp {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
        ty,
    }
}

/// If `expr` is statically decidable as a boolean (folds to a `LitBool`),
/// return `Some(b)`; otherwise `None` (data-dependent or non-boolean).
///
/// Drives R8 (drop a statically-true `Filter`) and R9 (replace a
/// statically-false `Filter` with `Op::Empty`).
#[must_use]
pub fn static_truth(expr: &Expr<'_>) -> Option<bool> {
    match partial_eval(expr) {
        Expr::LitBool(b) => Some(b),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smol_str::SmolStr;

    fn db() -> fossil_base::FossilDb {
        let system: std::sync::Arc<dyn fossil_base::System> =
            std::sync::Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    fn bool_ty(db: &dyn fossil_base::Db) -> fossil_hir::Ty<'_> {
        fossil_hir::Ty::new(
            db,
            fossil_hir::TyKind::Primitive(fossil_hir::Primitive::Bool),
        )
    }

    fn lit(s: &str) -> Expr<'static> {
        Expr::LitString(SmolStr::from(s))
    }

    fn col(c: &str) -> Expr<'static> {
        Expr::ColRef {
            source: SmolStr::default(),
            column: SmolStr::from(c),
        }
    }

    fn eq<'db>(db: &'db dyn fossil_base::Db, lhs: Expr<'db>, rhs: Expr<'db>) -> Expr<'db> {
        Expr::BinOp {
            op: CmpOp::Eq,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            ty: bool_ty(db),
        }
    }

    #[test]
    fn concat_of_literals_folds() {
        let e = Expr::Concat(Box::new(lit("a")), Box::new(lit("b")));
        assert_eq!(partial_eval(&e), lit("ab"));
    }

    #[test]
    fn nested_concat_of_literals_folds() {
        // ("a" ++ "b") ++ "c" → "abc"
        let inner = Expr::Concat(Box::new(lit("a")), Box::new(lit("b")));
        let e = Expr::Concat(Box::new(inner), Box::new(lit("c")));
        assert_eq!(partial_eval(&e), lit("abc"));
    }

    #[test]
    fn concat_with_colref_does_not_fold() {
        let e = Expr::Concat(Box::new(lit("a/")), Box::new(col("id")));
        // Stays a Concat (data-dependent right operand), but the left literal
        // is preserved verbatim.
        match partial_eval(&e) {
            Expr::Concat(l, r) => {
                assert_eq!(*l, lit("a/"));
                assert_eq!(*r, col("id"));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn eq_of_equal_string_literals_is_true() {
        let db = db();
        let e = eq(&db, lit("x"), lit("x"));
        assert_eq!(static_truth(&e), Some(true));
    }

    #[test]
    fn eq_of_distinct_string_literals_is_false() {
        let db = db();
        let e = eq(&db, lit("a"), lit("b"));
        assert_eq!(static_truth(&e), Some(false));
    }

    #[test]
    fn eq_with_colref_is_undecidable() {
        let db = db();
        let e = eq(&db, col("id"), lit("x"));
        assert_eq!(static_truth(&e), None);
    }

    #[test]
    fn ordered_comparators_fold_over_strings() {
        let db = db();
        // "a" < "b" → true
        let lt = Expr::BinOp {
            op: CmpOp::Lt,
            lhs: Box::new(lit("a")),
            rhs: Box::new(lit("b")),
            ty: bool_ty(&db),
        };
        assert_eq!(static_truth(&lt), Some(true));
        // "b" <= "a" → false
        let le = Expr::BinOp {
            op: CmpOp::Le,
            lhs: Box::new(lit("b")),
            rhs: Box::new(lit("a")),
            ty: bool_ty(&db),
        };
        assert_eq!(static_truth(&le), Some(false));
    }

    #[test]
    fn and_with_false_short_circuits() {
        let db = db();
        // (id == "x") AND false → false
        let e = Expr::BinOp {
            op: CmpOp::And,
            lhs: Box::new(eq(&db, col("id"), lit("x"))),
            rhs: Box::new(Expr::LitBool(false)),
            ty: bool_ty(&db),
        };
        assert_eq!(static_truth(&e), Some(false));
    }

    #[test]
    fn and_with_true_drops_the_constant() {
        let db = db();
        // true AND (id == "x") → (id == "x")  (still undecidable)
        let pred = eq(&db, col("id"), lit("x"));
        let e = Expr::BinOp {
            op: CmpOp::And,
            lhs: Box::new(Expr::LitBool(true)),
            rhs: Box::new(pred.clone()),
            ty: bool_ty(&db),
        };
        assert_eq!(partial_eval(&e), pred);
        assert_eq!(static_truth(&e), None);
    }

    #[test]
    fn or_with_true_short_circuits() {
        let db = db();
        // (id == "x") OR true → true
        let e = Expr::BinOp {
            op: CmpOp::Or,
            lhs: Box::new(eq(&db, col("id"), lit("x"))),
            rhs: Box::new(Expr::LitBool(true)),
            ty: bool_ty(&db),
        };
        assert_eq!(static_truth(&e), Some(true));
    }

    #[test]
    fn partial_eval_is_idempotent() {
        let db = db();
        // ("a" ++ "b" == "ab") AND ("x" < "y")  → folds to true, stable.
        let lhs = eq(
            &db,
            Expr::Concat(Box::new(lit("a")), Box::new(lit("b"))),
            lit("ab"),
        );
        let rhs = Expr::BinOp {
            op: CmpOp::Lt,
            lhs: Box::new(lit("x")),
            rhs: Box::new(lit("y")),
            ty: bool_ty(&db),
        };
        let e = Expr::BinOp {
            op: CmpOp::And,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            ty: bool_ty(&db),
        };
        let once = partial_eval(&e);
        let twice = partial_eval(&once);
        assert_eq!(once, twice, "partial_eval must be a normal form");
        assert_eq!(once, Expr::LitBool(true));
    }
}
