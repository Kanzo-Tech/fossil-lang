//! Type erasure — strip every type annotation off a [`MirGraph`] (SC#3).
//!
//! [`erase_types`] maps every [`Ty<'db>`](fossil_hir::Ty) carried by an
//! [`Op`] / [`Expr`] to a single interned SENTINEL type, leaving every
//! STRUCTURAL field (column names, op wiring, predicates' operator/operand
//! shape, literal values) untouched. The result is the "untyped projection" of
//! the typed MIR.
//!
//! # Why this mechanizes the conservative-extension claim (ADR-0013)
//!
//! `operator-algebra.md` §1/§9 frames Fossil's typed MIR as a *conservative
//! typed extension* of Min Oo & Hartig's untyped algebra: types gate
//! compile-time errors and drive the R7–R10 typed rewrites, but they carry NO
//! operational meaning. Codegen never reads a `Ty` (every `ty:` field is
//! `ty: _` in `render_expr` / the aggregate arm) so the generated SQL is
//! identical with or without the annotations.
//!
//! [`erase_types`] makes that testable: the property test
//! (`fossil-codegen/tests/type_preservation.rs`) asserts
//! `codegen(g).sql == codegen(erase_types(g)).sql` over the whole 30-mapping
//! corpus. If any codegen path ever started consuming a type, the erased
//! projection's SQL would diverge and the test would fail.
//!
//! # The sentinel
//!
//! Erased types become `TyKind::Unknown(InferenceId(u32::MAX))` — a single
//! interned handle distinct from every real type. It is an internal marker that
//! NEVER reaches codegen output (codegen ignores types), so invariant #8 ("no
//! `TyKind::Unknown` in generated SQL") is upheld: the sentinel lives only in
//! the erased MIR, not in any rendered SQL string.

use fossil_hir::{InferenceId, Ty, TyKind};

use crate::graph::MirGraph;
use crate::op::{AggSpec, Expr, Op, VProp};

/// The interned erase sentinel — `TyKind::Unknown(InferenceId(u32::MAX))`.
///
/// Distinct from any real inference variable (the checker allocates ids from 0
/// upward) and from every surface type. Interned once per `db`.
fn sentinel(db: &dyn fossil_base::Db) -> Ty<'_> {
    Ty::new(db, TyKind::Unknown(InferenceId(u32::MAX)))
}

/// Erase every type annotation in `g`, returning a fresh [`MirGraph`] whose ops
/// are structurally identical but whose every `Ty` is the [`sentinel`].
///
/// Proves codegen never reads types for operational semantics: the erased graph
/// must codegen to byte-identical SQL (SC#3 — see [module docs](self)).
#[must_use]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the interning frame
pub fn erase_types<'db>(db: &'db dyn fossil_base::Db, g: MirGraph<'db>) -> MirGraph<'db> {
    let s = sentinel(db);
    let ops: Vec<Op<'db>> = g.ops(db).iter().map(|op| erase_op(op, s)).collect();
    MirGraph::new(db, ops)
}

/// Erase the type annotations on a single op, preserving every structural field.
fn erase_op<'db>(op: &Op<'db>, s: Ty<'db>) -> Op<'db> {
    match op {
        // The ONLY op carrying a `Ty` directly is `Source.row_type`.
        Op::Source {
            uri,
            format,
            row_type: _,
            binding,
        } => Op::Source {
            uri: uri.clone(),
            format: format.clone(),
            row_type: s,
            binding: binding.clone(),
        },
        // These ops carry NO `Ty` themselves; only their `Expr` fields might.
        Op::Project { input, cols } => Op::Project {
            input: *input,
            cols: cols.clone(),
        },
        Op::Extend { input, field, expr } => Op::Extend {
            input: *input,
            field: field.clone(),
            expr: erase_expr(expr, s),
        },
        Op::Rename { input, old, new } => Op::Rename {
            input: *input,
            old: old.clone(),
            new: new.clone(),
        },
        Op::Filter { input, pred } => Op::Filter {
            input: *input,
            pred: erase_expr(pred, s),
        },
        Op::Join {
            left,
            right,
            on,
            kind,
            left_name,
            right_name,
        } => Op::Join {
            left: *left,
            right: *right,
            on: erase_expr(on, s),
            kind: *kind,
            left_name: left_name.clone(),
            right_name: right_name.clone(),
        },
        Op::Union { left, right } => Op::Union {
            left: *left,
            right: *right,
        },
        Op::GroupBy { input, keys } => Op::GroupBy {
            input: *input,
            keys: keys.clone(),
        },
        Op::Aggregate { input, aggs } => Op::Aggregate {
            input: *input,
            aggs: aggs.iter().map(|a| erase_agg(a, s)).collect(),
        },
        Op::Distinct { input, by } => Op::Distinct {
            input: *input,
            by: by.clone(),
        },
        Op::EmitVertex {
            input,
            type_name,
            rdf_type,
            id,
            dedup,
            props,
        } => Op::EmitVertex {
            input: *input,
            type_name: type_name.clone(),
            rdf_type: rdf_type.clone(),
            id: erase_expr(id, s),
            dedup: *dedup,
            props: props.iter().map(|p| erase_vprop(p, s)).collect(),
        },
        Op::EmitEdge {
            input,
            edge_type,
            rdf_uri,
            src_type,
            dst_type,
            src_id,
            dst_id,
            single_valued,
        } => Op::EmitEdge {
            input: *input,
            edge_type: edge_type.clone(),
            rdf_uri: rdf_uri.clone(),
            src_type: src_type.clone(),
            dst_type: dst_type.clone(),
            src_id: erase_expr(src_id, s),
            dst_id: erase_expr(dst_id, s),
            single_valued: *single_valued,
        },
        Op::Sink { input, sink } => Op::Sink {
            input: *input,
            sink: *sink,
        },
        Op::Empty { schema } => Op::Empty {
            schema: schema.clone(),
        },
    }
}

/// Erase the `ty` on a [`VProp`], preserving the name / predicate / cardinality
/// and erasing the value expression's annotations.
fn erase_vprop<'db>(p: &VProp<'db>, s: Ty<'db>) -> VProp<'db> {
    VProp {
        name: p.name.clone(),
        value: erase_expr(&p.value, s),
        ty: s,
        rdf_uri: p.rdf_uri.clone(),
        single_valued: p.single_valued,
    }
}

/// Erase the `ty` on an [`AggSpec`], preserving the function + field names.
fn erase_agg<'db>(spec: &AggSpec<'db>, s: Ty<'db>) -> AggSpec<'db> {
    AggSpec {
        out_field: spec.out_field.clone(),
        agg_fn: spec.agg_fn,
        in_field: spec.in_field.clone(),
        ty: s,
    }
}

/// Erase every `ty` annotation inside an [`Expr`] tree, preserving operators,
/// operand shape, column references, and literal values.
fn erase_expr<'db>(expr: &Expr<'db>, s: Ty<'db>) -> Expr<'db> {
    match expr {
        Expr::LitString(v) => Expr::LitString(v.clone()),
        Expr::LitBool(b) => Expr::LitBool(*b),
        Expr::ColRef { source, column } => Expr::ColRef {
            source: source.clone(),
            column: column.clone(),
        },
        Expr::Concat(lhs, rhs) => {
            Expr::Concat(Box::new(erase_expr(lhs, s)), Box::new(erase_expr(rhs, s)))
        }
        Expr::Call { func, args, ty: _ } => Expr::Call {
            func: func.clone(),
            args: args.iter().map(|a| erase_expr(a, s)).collect(),
            ty: s,
        },
        Expr::BinOp {
            op,
            lhs,
            rhs,
            ty: _,
        } => Expr::BinOp {
            op: *op,
            lhs: Box::new(erase_expr(lhs, s)),
            rhs: Box::new(erase_expr(rhs, s)),
            ty: s,
        },
        Expr::Assert {
            name,
            span_line,
            inner,
        } => Expr::Assert {
            name: name.clone(),
            span_line: *span_line,
            inner: Box::new(erase_expr(inner, s)),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
    use smol_str::SmolStr;

    use super::*;
    use crate::op::{CmpOp, SourceFormat};

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    #[salsa::input]
    struct Trigger {
        case: u8,
    }

    /// `MirGraph::new` / `Record::new` are tracked structs — they may only be
    /// created inside a tracked frame. This assertion body runs the erase
    /// invariants from within such a frame. `case == 0` runs the
    /// structural-only check; `case == 1` runs the idempotency check.
    #[salsa::tracked]
    fn run_erase_checks(db: &dyn fossil_base::Db, trigger: Trigger) {
        let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
        let s = sentinel(db);

        if trigger.case(db) == 0 {
            let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
            let row = Ty::new(
                db,
                TyKind::Record(Record::new(
                    db,
                    vec![RecordField {
                        name: SmolStr::new_static("id"),
                        ty: string_ty,
                    }],
                )),
            );
            let ops = vec![
                Op::Source {
                    uri: SmolStr::new_static("examples/users.csv"),
                    format: SourceFormat::Csv,
                    row_type: row,
                    binding: SmolStr::new_static("users"),
                },
                Op::Filter {
                    input: 0,
                    pred: Expr::BinOp {
                        op: CmpOp::Eq,
                        lhs: Box::new(Expr::ColRef {
                            source: SmolStr::new_static("users"),
                            column: SmolStr::new_static("id"),
                        }),
                        rhs: Box::new(Expr::LitString(SmolStr::new_static("1"))),
                        ty: bool_ty,
                    },
                },
            ];
            let g = MirGraph::new(db, ops.clone());
            let erased = erase_types(db, g);
            let erased_ops = erased.ops(db);

            // Source: structural fields identical, row_type → sentinel.
            match (&ops[0], &erased_ops[0]) {
                (
                    Op::Source {
                        uri: u0,
                        format: f0,
                        ..
                    },
                    Op::Source {
                        uri: u1,
                        format: f1,
                        row_type: rt,
                        ..
                    },
                ) => {
                    assert_eq!(u0, u1);
                    assert_eq!(f0, f1);
                    assert_eq!(*rt, s, "row_type must be the erase sentinel");
                    assert_ne!(*rt, row, "row_type must differ from the original");
                }
                _ => panic!("expected Source"),
            }

            // Filter: predicate shape identical, BinOp.ty → sentinel.
            match &erased_ops[1] {
                Op::Filter {
                    input,
                    pred: Expr::BinOp { op, lhs, rhs, ty },
                } => {
                    assert_eq!(*input, 0);
                    assert_eq!(*op, CmpOp::Eq);
                    assert!(matches!(**lhs, Expr::ColRef { .. }));
                    assert!(matches!(**rhs, Expr::LitString(_)));
                    assert_eq!(*ty, s, "BinOp.ty must be the erase sentinel");
                }
                _ => panic!("expected Filter(BinOp)"),
            }
        } else {
            // Idempotency: erasing an already-erased graph is a no-op.
            let ops = vec![Op::Source {
                uri: SmolStr::new_static("examples/users.csv"),
                format: SourceFormat::Csv,
                row_type: string_ty,
                binding: SmolStr::new_static("users"),
            }];
            let g = MirGraph::new(db, ops);
            let once = erase_types(db, g);
            let twice = erase_types(db, once);
            assert_eq!(once.ops(db), twice.ops(db));
        }
    }

    /// `erase_types` changes ONLY the `ty` fields — structural fields (op kind,
    /// wiring, column names, literal values, operators) are identical.
    #[test]
    fn erase_changes_only_type_fields() {
        let db = db();
        let trigger = Trigger::new(&db, 0);
        run_erase_checks(&db, trigger);
    }

    /// Erasing an already-erased graph is a no-op (idempotent): the sentinel
    /// erases to itself.
    #[test]
    fn erase_is_idempotent() {
        let db = db();
        let trigger = Trigger::new(&db, 1);
        run_erase_checks(&db, trigger);
    }
}
