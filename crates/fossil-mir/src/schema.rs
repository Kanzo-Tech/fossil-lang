//! Structural MIR helpers: [`schema_of`] and [`free_cols`].
//!
//! These are plain-Rust functions (NOT `#[salsa::tracked]` queries), so they
//! do NOT affect `MAX_PER_MAPPING_FAN_OUT` — a function with no Salsa key
//! cannot be re-executed by an invalidation, which is a property of the
//! signature and not of the body. `tests/fan_out.rs` measures the fan-out of
//! [`crate::lower_to_mir_pg`] and does not put these two in its loop, so the
//! claim is read off the `fn` above, not off a count. They reason about the
//! column schema flowing through a [`crate::graph::MirGraph`] and the free
//! column references inside an [`Expr`].
//!
//! # `schema_of` takes `&dyn fossil_base::Db`
//!
//! `schema_of` takes `db` because the [`Op::Source`] variant's `row_type` is an
//! interned [`fossil_hir::Ty`] handle — deref-ing it to read the `Record`
//! field names requires the database. This is the single authoritative
//! signature: `schema_of(db, ops, idx)`. There is NO no-`db` variant (it would
//! be unable to read the source row schema). Because it is a plain function and
//! not a tracked query, taking `db` here does not widen the per-mapping
//! fan-out.

use std::collections::BTreeSet;

use fossil_hir::{Ty, TyKind};
use smol_str::SmolStr;

use crate::op::{Expr, Op};

/// Output column schema of the op at `idx`.
///
/// Computed by structural induction over the topo-ordered DAG. That induction
/// is what makes the algebra's preservation property hold: if every operator's
/// input schema matches its signature, every node in the plan carries a
/// well-typed output schema. Returns column names in schema order.
///
/// Per-operator rules:
/// - `Source` → field names of `row_type` (deref the interned `Record`)
/// - `Project` → `cols`
/// - `Extend` → input schema ∪ `{field}` (field appended if not already present)
/// - `Rename` → input schema with `old` → `new`
/// - `Filter` / `Distinct` → input schema unchanged
/// - `Join` → left schema ++ right schema, whole. A shared name used to be a
///   compile error, and is not any more: the join no longer flattens two rows
///   into one, every reference is written qualified, so two sources with a
///   column of the same name are legal. This concatenation can therefore
///   produce a duplicate name, and nothing here breaks the tie — these are bare
///   names, and the qualifier that does break the tie lives on the `ColRef`
///   that reads them, not in this list.
/// - `Union` → left schema (asserted equal to right in debug builds)
/// - `GroupBy` → `keys`
/// - `Aggregate` → input schema ∪ agg `out_field`s
/// - `EmitVertex` / `EmitEdge` / `Sink` → input schema unchanged (terminal-ish)
/// - `Empty` → its declared `schema`
///
/// An out-of-range `idx` (or an input index that points past the slice)
/// yields an empty schema rather than panicking: a caller may hold a
/// half-built or hand-constructed graph, and a panic there would take the LSP
/// with it.
#[must_use]
pub fn schema_of(db: &dyn fossil_base::Db, ops: &[Op<'_>], idx: usize) -> Vec<SmolStr> {
    let Some(op) = ops.get(idx) else {
        return Vec::new();
    };
    match op {
        Op::Source { row_type, .. } => record_field_names(db, *row_type),
        Op::Project { cols, .. } => cols.iter().map(|c| c.column.clone()).collect(),
        Op::Extend { input, field, .. } => {
            let mut schema = schema_of(db, ops, *input);
            if !schema.contains(field) {
                schema.push(field.clone());
            }
            schema
        }
        Op::Rename { input, old, new } => {
            let mut schema = schema_of(db, ops, *input);
            for col in &mut schema {
                if col == old {
                    *col = new.clone();
                }
            }
            schema
        }
        // Schema-preserving operators: pass the input schema through unchanged.
        // EmitVertex/EmitEdge are terminal-ish at the MIR-schema level (the
        // PG/dense-id shaping is the backend's, not the logical schema).
        Op::Filter { input, .. }
        | Op::Distinct { input, .. }
        | Op::EmitVertex { input, .. }
        | Op::EmitEdge { input, .. }
        | Op::Sink { input, .. } => schema_of(db, ops, *input),
        // `fila(izq) ++ fila(der)`, whole. This dropped the right side's copy of
        // the key between 2026-08-07 and 2026-08-19, because the executor did:
        // the join was `USING (k)`, so a schema that kept the second `k`
        // described a column nobody would find. The executor no longer
        // identifies anything — a join relates two qualified relations and both
        // keep every column — so neither does this.
        Op::Join { left, right, .. } => {
            let mut schema = schema_of(db, ops, left.input);
            schema.extend(schema_of(db, ops, right.input));
            schema
        }
        Op::Union { left, right } => {
            let left_schema = schema_of(db, ops, *left);
            debug_assert_eq!(
                left_schema,
                schema_of(db, ops, *right),
                "Union requires both inputs to share a schema"
            );
            left_schema
        }
        Op::GroupBy { keys, .. } => keys.clone(),
        Op::Aggregate { input, aggs } => {
            let mut schema = schema_of(db, ops, *input);
            for agg in aggs {
                if !schema.contains(&agg.out_field) {
                    schema.push(agg.out_field.clone());
                }
            }
            schema
        }
        Op::Empty { schema } => schema.clone(),
    }
}

/// Free column references in an expression — the columns a predicate reads,
/// which is what decides whether it may be evaluated against a given schema.
///
/// Walks the [`Expr`] recursively collecting `ColRef.column` names;
/// `LitString` / `LitBool` contribute nothing; `Concat` / `Call` / `BinOp` /
/// `Assert` recurse into their children.
#[must_use]
pub fn free_cols(expr: &Expr<'_>) -> BTreeSet<SmolStr> {
    let mut acc = BTreeSet::new();
    collect_free_cols(expr, &mut acc);
    acc
}

fn collect_free_cols(expr: &Expr<'_>, acc: &mut BTreeSet<SmolStr>) {
    match expr {
        Expr::LitString(_) | Expr::LitBool(_) | Expr::LitInt(_) | Expr::LitFloat(_) => {}
        Expr::ColRef { column, .. } => {
            acc.insert(column.clone());
        }
        Expr::Concat(lhs, rhs) | Expr::BinOp { lhs, rhs, .. } => {
            collect_free_cols(lhs, acc);
            collect_free_cols(rhs, acc);
        }
        // The column being tested is read, exactly as it would be by any other
        // comparison — `IS NULL` is not a way of not reading it.
        Expr::IsNull { operand, .. } => collect_free_cols(operand, acc),
        Expr::Call { args, .. } => {
            for arg in args {
                collect_free_cols(arg, acc);
            }
        }
        Expr::Ternary {
            cond,
            then,
            otherwise,
            ..
        } => {
            collect_free_cols(cond, acc);
            collect_free_cols(then, acc);
            collect_free_cols(otherwise, acc);
        }
        Expr::Assert { inner, .. } | Expr::UnaryOp { operand: inner, .. } => {
            collect_free_cols(inner, acc);
        }
    }
}

/// Deref an interned `Record` row type to its field names. A non-`Record`
/// `row_type` (should not occur for a well-formed `Source`) yields an empty
/// schema.
fn record_field_names(db: &dyn fossil_base::Db, row_type: Ty<'_>) -> Vec<SmolStr> {
    match row_type.kind(db) {
        TyKind::Record(rec) => rec.fields(db).iter().map(|f| f.name.clone()).collect(),
        _ => Vec::new(),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::op::{SinkRef, SourceFormat, VProp};
    use fossil_graph_schema::Primitive;
    use fossil_hir::BinOp;
    use fossil_hir::ty::{Record, RecordField};
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    fn string_record<'db>(db: &'db dyn fossil_base::Db, names: &[&str]) -> Ty<'db> {
        let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
        let fields: Vec<RecordField<'db>> = names
            .iter()
            .map(|n| RecordField {
                name: SmolStr::from(*n),
                ty: string_ty,
            })
            .collect();
        let rec = Record::new(db, fields);
        Ty::new(db, TyKind::Record(rec))
    }

    /// PG-canonical operators: `EmitVertex` / `EmitEdge` construct and are
    /// schema-passthrough at the MIR level (the PG/dense-id shaping is the
    /// backend's, not the logical schema's).
    #[test]
    fn schema_of_emit_vertex_edge_passthrough() {
        let db = db();
        let row_type = string_record(&db, &["id", "name"]);
        let string_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
        let iri = || Expr::ColRef {
            source: SmolStr::default(),
            column: SmolStr::new_static("iri"),
        };
        let ops = vec![
            Op::Source {
                uri: SmolStr::new_static("users.csv"),
                format: SourceFormat::Csv,
                row_type,
                binding: SmolStr::new_static("users"),
            },
            Op::Extend {
                input: 0,
                field: SmolStr::new_static("iri"),
                expr: Expr::LitString(SmolStr::new_static("x")),
            },
            Op::EmitVertex {
                input: 1,
                type_name: SmolStr::new_static("Person"),
                rdf_type: Some(SmolStr::new_static("https://example.org/Person")),
                id: iri(),
                dedup: true,
                props: vec![VProp {
                    name: SmolStr::new_static("name"),
                    value: Expr::ColRef {
                        source: SmolStr::new_static("users"),
                        column: SmolStr::new_static("name"),
                    },
                    ty: string_ty,
                    rdf_uri: Some(SmolStr::new_static("https://example.org/name")),
                    single_valued: true,
                }],
            },
            Op::EmitEdge {
                input: 2,
                edge_type: SmolStr::new_static("knows"),
                rdf_uri: Some(SmolStr::new_static("https://example.org/knows")),
                src_type: SmolStr::new_static("Person"),
                dst_type: SmolStr::new_static("Person"),
                src_id: iri(),
                dst_id: Expr::ColRef {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("friend"),
                },
                single_valued: false,
            },
            Op::Sink {
                input: 3,
                sink: SinkRef::GraphAr,
            },
        ];
        // Both Emit ops + Sink pass the input schema (post-Extend) through.
        let after_extend = schema_of(&db, &ops, 1);
        assert_eq!(
            schema_of(&db, &ops, 2),
            after_extend,
            "EmitVertex passthrough"
        );
        assert_eq!(
            schema_of(&db, &ops, 3),
            after_extend,
            "EmitEdge passthrough"
        );
        assert_eq!(schema_of(&db, &ops, 4), after_extend, "Sink passthrough");
    }

    #[test]
    fn schema_of_rename_and_project() {
        let db = db();
        let row_type = string_record(&db, &["id", "name"]);
        let ops = vec![
            Op::Source {
                uri: SmolStr::new_static("u.csv"),
                format: SourceFormat::Csv,
                row_type,
                binding: SmolStr::new_static("u"),
            },
            Op::Rename {
                input: 0,
                old: SmolStr::new_static("name"),
                new: SmolStr::new_static("full_name"),
            },
            Op::Project {
                input: 1,
                cols: vec![crate::op::ProjectedColumn {
                    source: SmolStr::new_static("u"),
                    column: SmolStr::new_static("full_name"),
                }],
            },
        ];
        assert_eq!(
            schema_of(&db, &ops, 1),
            vec![SmolStr::new_static("id"), SmolStr::new_static("full_name")]
        );
        assert_eq!(
            schema_of(&db, &ops, 2),
            vec![SmolStr::new_static("full_name")]
        );
    }

    #[test]
    fn free_cols_over_binop_eq_colref_litstring() {
        // BinOp(Eq, ColRef{status}, LitString) → {status}
        let db = db();
        let bool_ty = Ty::new(&db, TyKind::Primitive(Primitive::Bool));
        let expr = Expr::BinOp {
            op: BinOp::Eq,
            lhs: Box::new(Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("status"),
            }),
            rhs: Box::new(Expr::LitString(SmolStr::new_static("active"))),
            ty: bool_ty,
        };
        let free = free_cols(&expr);
        assert_eq!(free.len(), 1);
        assert!(free.contains(&SmolStr::new_static("status")));
    }

    #[test]
    fn free_cols_recurses_through_concat_and_call() {
        let db = db();
        let string_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
        // concat(call(upper, ColRef{a}), ColRef{b})
        let expr = Expr::Concat(
            Box::new(Expr::Call {
                func: SmolStr::new_static("upper"),
                args: vec![Expr::ColRef {
                    source: SmolStr::default(),
                    column: SmolStr::new_static("a"),
                }],
                ty: string_ty,
            }),
            Box::new(Expr::ColRef {
                source: SmolStr::default(),
                column: SmolStr::new_static("b"),
            }),
        );
        let free = free_cols(&expr);
        assert_eq!(free.len(), 2);
        assert!(free.contains(&SmolStr::new_static("a")));
        assert!(free.contains(&SmolStr::new_static("b")));
    }

    #[test]
    fn free_cols_empty_for_literals() {
        assert!(free_cols(&Expr::LitString(SmolStr::new_static("x"))).is_empty());
        assert!(free_cols(&Expr::LitBool(true)).is_empty());
    }
}
