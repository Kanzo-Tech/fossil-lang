//! Structural MIR helpers: [`schema_of`] and [`free_cols`].
//!
//! These are plain-Rust functions (NOT `#[salsa::tracked]` queries), so they
//! do NOT affect `MAX_PER_MAPPING_FAN_OUT`. The rewriting engine (plans
//! 04-02/04-03) and codegen (plans 04-04/04-05) call them to reason about the
//! column schema flowing through a [`crate::graph::MirGraph`] and the free
//! column references inside an [`Expr`] (for the R3/R5 `free(p)` guards).
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
/// Computed by structural induction over the topo-ordered DAG
/// (operator-algebra.md §3). Returns column names in schema order.
///
/// Per-operator rules (operator-algebra.md §3):
/// - `Source` → field names of `row_type` (deref the interned `Record`)
/// - `Project` → `cols`
/// - `Extend` → input schema ∪ `{field}` (field appended if not already present)
/// - `Rename` → input schema with `old` → `new`
/// - `Filter` / `Distinct` → input schema unchanged
/// - `Join` → left schema ∪ right schema (v0.1 unions the names; collisions are
///   resolved by the `left_name` / `right_name` qualifiers in codegen)
/// - `Union` → left schema (asserted equal to right in debug builds)
/// - `GroupBy` → `keys`
/// - `Aggregate` → input schema ∪ agg `out_field`s
/// - `TripleEmit` / `Sink` → input schema unchanged (terminal-ish)
/// - `Empty` → its declared `schema`
///
/// An out-of-range `idx` (or an input index that points past the slice)
/// yields an empty schema rather than panicking — callers in the rewriting
/// engine handle malformed intermediate graphs gracefully.
#[must_use]
pub fn schema_of(db: &dyn fossil_base::Db, ops: &[Op<'_>], idx: usize) -> Vec<SmolStr> {
    let Some(op) = ops.get(idx) else {
        return Vec::new();
    };
    match op {
        Op::Source { row_type, .. } => record_field_names(db, *row_type),
        Op::Project { cols, .. } => cols.clone(),
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
        Op::Filter { input, .. }
        | Op::Distinct { input, .. }
        | Op::TripleEmit { input, .. }
        | Op::Sink { input, .. } => schema_of(db, ops, *input),
        Op::Join { left, right, .. } => {
            let mut schema = schema_of(db, ops, *left);
            schema.extend(schema_of(db, ops, *right));
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

/// Free column references in an expression (for the R3/R5 `free(p)` guards).
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
        Expr::LitString(_) | Expr::LitBool(_) => {}
        Expr::ColRef { column, .. } => {
            acc.insert(column.clone());
        }
        Expr::Concat(lhs, rhs) | Expr::BinOp { lhs, rhs, .. } => {
            collect_free_cols(lhs, acc);
            collect_free_cols(rhs, acc);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_free_cols(arg, acc);
            }
        }
        Expr::Assert { inner, .. } => collect_free_cols(inner, acc),
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
    use crate::op::{CmpOp, SinkRef, SourceFormat};
    use fossil_hir::ty::{Primitive, Record, RecordField};
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
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

    #[test]
    fn schema_of_source_extend_triple_emit_chain() {
        let db = db();
        let row_type = string_record(&db, &["id", "name"]);
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
            Op::TripleEmit {
                input: 1,
                subject: Expr::ColRef {
                    source: SmolStr::default(),
                    column: SmolStr::new_static("iri"),
                },
                predicate: SmolStr::new_static("https://example.org/name"),
                object: Expr::ColRef {
                    source: SmolStr::new_static("users"),
                    column: SmolStr::new_static("name"),
                },
                graph: None,
            },
            Op::Sink {
                input: 2,
                sink: SinkRef::GraphAr,
            },
        ];

        // Source row schema is the record field names.
        assert_eq!(
            schema_of(&db, &ops, 0),
            vec![SmolStr::new_static("id"), SmolStr::new_static("name")]
        );
        // Extend appends the `iri` column.
        assert_eq!(
            schema_of(&db, &ops, 1),
            vec![
                SmolStr::new_static("id"),
                SmolStr::new_static("name"),
                SmolStr::new_static("iri")
            ]
        );
        // TripleEmit and Sink pass the input schema through unchanged.
        assert_eq!(schema_of(&db, &ops, 2), schema_of(&db, &ops, 1));
        assert_eq!(schema_of(&db, &ops, 3), schema_of(&db, &ops, 1));
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
                cols: vec![SmolStr::new_static("full_name")],
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
            op: CmpOp::Eq,
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
