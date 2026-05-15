//! HIR → MIR lowering for the Phase 1 canonical example.
//!
//! Consumes [`fossil_hir::HirMapping`] and emits a 4-node [`MirGraph`]:
//! `Source → Extend(iri = ...) → TripleEmit → Sink(GraphAr)`.
//!
//! # Phase 1 hardcoded paths (deferred work flagged inline)
//!
//! - **Source URI** is `"examples/users.csv"` regardless of the source
//!   binding's actual `io.csv("...")` argument. Phase 5 STDL-06 promotes this
//!   to a real registry lookup against the `SOURCE_DEF` CST.
//! - **Row type** is `Record({id: String, name: String})`. Phase 3 CORE-05
//!   replaces this with a CSVW-derived row type via forward propagation.
//! - **IRI-template prefix expansion** is hardcoded for the single prefix
//!   `ex:` → `https://example.org/`. Phase 4 lifts template parsing from
//!   raw-text dispatch into a proper expression tree on the HIR side.
//!
//! # Public Salsa query signature (Phase 2-9 contract — locked)
//!
//! ```ignore
//! #[salsa::tracked]
//! pub fn lower_to_mir<'db>(
//!     db: &'db dyn fossil_base::Db,
//!     mapping: fossil_hir::MappingLoc<'db>,
//! ) -> MirGraph<'db>;
//! ```

use fossil_hir::def_map::def_map;
use fossil_hir::lower::lower_to_hir;
use fossil_hir::ty::RecordField;
use fossil_hir::{HirExpr, HirMapping, MappingLoc, Primitive, PropertyKey, Record, Ty, TyKind};
use smol_str::SmolStr;

use crate::graph::MirGraph;
use crate::op::{ExprLowered, Op, SinkRef, SourceFormat};

/// Lower one [`fossil_hir::MappingLoc`] to a [`MirGraph`] of 4 ops:
/// `Source → Extend(iri) → TripleEmit → Sink(GraphAr)`.
///
/// Phase 1 emits exactly one [`Op::TripleEmit`] per mapping (the single
/// `ex:name = .name` property). Phase 2 (CORE-08) generalises to multi-
/// property mappings (one `TripleEmit` per non-`iri` property, all sharing the
/// same `Extend(iri)` upstream).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn lower_to_mir<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> MirGraph<'db> {
    let file = mapping.file(db);

    // `MappingLoc::index` is the position among ALL top-level CST children
    // (PREFIX_DECL, SOURCE_DEF, MAPPING all share the index space — see
    // `fossil_hir::def_map::def_map`). The `lower_to_hir` mappings list, by
    // contrast, is filtered to MAPPING-only and is densely indexed 0..N. Both
    // lists are produced by walking CST children in the same CST order, so
    // we recover the dense index by finding `mapping`'s position in
    // `def_map(file).mappings()` (the same MappingLoc identity is interned
    // across queries).
    let dm = def_map(db, file);
    let mapping_locs = dm.mappings(db);
    let Some(dense_idx) = mapping_locs.iter().position(|loc| *loc == mapping) else {
        // Foreign MappingLoc — emit an empty graph rather than panicking.
        // Phase 3 (CORE-04..07) plumbs ErrorGuaranteed propagation.
        return MirGraph::new(db, Vec::new());
    };
    let hir = lower_to_hir(db, file);
    let mappings = hir.mappings(db);
    let Some(m) = mappings.get(dense_idx) else {
        return MirGraph::new(db, Vec::new());
    };

    let row_type = phase1_row_type(db);
    let mut ops: Vec<Op<'db>> = Vec::with_capacity(4);

    // 0: Source — read users.csv as Record({id: String, name: String})
    ops.push(Op::Source {
        uri: SmolStr::from("examples/users.csv"),
        format: SourceFormat::Csv,
        row_type,
    });

    // 1: Extend — attach the IRI template result as a column named "iri"
    let iri_expr = lower_iri_property(m).unwrap_or_else(|| {
        // No `iri = ...` property in the mapping; emit an empty literal to
        // keep the op-count invariant. Phase 3 (CORE-04..07) reports this as
        // a type error against the ShEx output descriptor.
        ExprLowered::LitString(SmolStr::default())
    });
    ops.push(Op::Extend {
        input: 0,
        field: SmolStr::new_static("iri"),
        expr: iri_expr,
    });

    // 2: TripleEmit — for the single (PrefixedName, FieldRef) property pair.
    let (predicate, object_col) = lower_first_predicate_property(m)
        .unwrap_or_else(|| (SmolStr::default(), SmolStr::default()));
    ops.push(Op::TripleEmit {
        input: 1,
        subject_col: SmolStr::new_static("iri"),
        predicate,
        object_col,
    });

    // 3: Sink — GraphAr terminal
    ops.push(Op::Sink {
        input: 2,
        sink: SinkRef::GraphAr,
    });

    MirGraph::new(db, ops)
}

/// Build the Phase 1 row type: `Record({id: String, name: String})`.
///
/// Phase 3 (CORE-05) replaces this stub with a CSVW-driven derivation from
/// the `SOURCE_DEF` descriptor.
fn phase1_row_type(db: &dyn fossil_base::Db) -> Ty<'_> {
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    let record = Record::new(
        db,
        vec![
            RecordField {
                name: SmolStr::new_static("id"),
                ty: string_ty,
            },
            RecordField {
                name: SmolStr::new_static("name"),
                ty: string_ty,
            },
        ],
    );
    Ty::new(db, TyKind::Record(record))
}

/// Find the `iri = ...` property in a mapping and lower its template value
/// to a concat-chain of [`ExprLowered`].
fn lower_iri_property(m: &HirMapping) -> Option<ExprLowered> {
    let prop = m
        .properties
        .iter()
        .find(|p| matches!(p.key, PropertyKey::Iri))?;
    let HirExpr::Template(raw) = &prop.value else {
        return None;
    };
    Some(lower_iri_template_phase1(raw, &m.source_binding))
}

/// Find the first `<prefix>:<local> = <field-ref>` property and return
/// `(predicate_iri, object_col)` for the `TripleEmit`.
fn lower_first_predicate_property(m: &HirMapping) -> Option<(SmolStr, SmolStr)> {
    for prop in &m.properties {
        if let PropertyKey::PrefixedName { iri } = &prop.key
            && let HirExpr::FieldRef(field) = &prop.value
        {
            return Some((iri.clone(), field.clone()));
        }
    }
    None
}

/// Phase 1 IRI-template lowering. Parses the raw template token text
/// (including surrounding backticks and `${...}` placeholders) and emits a
/// left-leaning [`ExprLowered::Concat`] chain of literal segments and column
/// references.
///
/// Recognised placeholder forms:
/// - `${ex:}` → literal `https://example.org/` (the only Phase 1 prefix)
/// - `${.field}` → [`ExprLowered::ColRef`] against the mapping's source binding
///
/// Phase 4 lifts template parsing into a proper HIR-side expression tree;
/// the resolved-prefix table will be threaded through, so the hardcoded `ex:`
/// branch goes away.
fn lower_iri_template_phase1(raw: &str, source_binding: &SmolStr) -> ExprLowered {
    // Strip the surrounding backticks (the HIR keeps them on the raw token).
    let inner = raw.trim_start_matches('`').trim_end_matches('`');

    let mut parts: Vec<ExprLowered> = Vec::new();
    let mut cursor = 0usize;
    while cursor < inner.len() {
        // Find the next `${` placeholder start.
        let Some(open_off) = inner[cursor..].find("${") else {
            // No more placeholders — push the remaining literal tail.
            let tail = &inner[cursor..];
            if !tail.is_empty() {
                parts.push(ExprLowered::LitString(SmolStr::from(tail)));
            }
            break;
        };
        let open = cursor + open_off;
        // Push the literal segment before the placeholder.
        if open > cursor {
            let lit = &inner[cursor..open];
            parts.push(ExprLowered::LitString(SmolStr::from(lit)));
        }
        // Find the matching `}`.
        let after_open = open + 2; // skip "${"
        let Some(close_off) = inner[after_open..].find('}') else {
            // Unterminated placeholder; treat the rest as a literal tail.
            let tail = &inner[open..];
            parts.push(ExprLowered::LitString(SmolStr::from(tail)));
            break;
        };
        let close = after_open + close_off;
        let placeholder = &inner[after_open..close];
        parts.push(lower_placeholder_phase1(placeholder, source_binding));
        cursor = close + 1; // skip past `}`
    }

    fold_concat_left(parts)
}

/// Lower one placeholder body (the text between `${` and `}`).
fn lower_placeholder_phase1(body: &str, source_binding: &SmolStr) -> ExprLowered {
    // Recognised forms (in priority order):
    //   `.field` → ColRef against the mapping's source binding
    //   `ex:`    → hardcoded Phase 1 prefix expansion (Phase 4 generalises)
    //   anything else → echo the placeholder back as a literal for debugging
    //                  (Phase 3 reports this as a diagnostic against IriTemplate)
    match body.strip_prefix('.') {
        Some(field) => ExprLowered::ColRef {
            source: source_binding.clone(),
            column: SmolStr::from(field),
        },
        None if body == "ex:" => {
            ExprLowered::LitString(SmolStr::new_static("https://example.org/"))
        }
        None => ExprLowered::LitString(SmolStr::from(format!("${{{body}}}"))),
    }
}

/// Fold a list of expression parts into a left-leaning Concat chain with
/// adjacent-literal fusion: `[Lit("a"), Lit("b"), Col]` → `Concat(Lit("ab"), Col)`.
///
/// Fusion is required for the Phase 1 codegen to produce the snapshot SQL
/// in RESEARCH.md Example 13 character-for-character (the snapshot expects
/// `'https://example.org/user/'`, not `'https://example.org/' || 'user/'`).
fn fold_concat_left(parts: Vec<ExprLowered>) -> ExprLowered {
    let mut fused: Vec<ExprLowered> = Vec::with_capacity(parts.len());
    for part in parts {
        match (fused.last_mut(), &part) {
            (Some(ExprLowered::LitString(prev)), ExprLowered::LitString(next)) => {
                let merged = SmolStr::from(format!("{prev}{next}"));
                *prev = merged;
            }
            _ => fused.push(part),
        }
    }
    if fused.is_empty() {
        return ExprLowered::LitString(SmolStr::default());
    }
    let mut iter = fused.into_iter();
    let mut acc = iter.next().expect("non-empty after the empty check above");
    for next in iter {
        acc = ExprLowered::Concat(Box::new(acc), Box::new(next));
    }
    acc
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use fossil_hir::def_map::def_map;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    #[test]
    fn lower_to_mir_for_hello_produces_4_ops() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("hello has one mapping");
        let mir = lower_to_mir(&db, mapping);
        let ops = mir.ops(&db);
        assert_eq!(ops.len(), 4, "expected 4 ops, got {}", ops.len());

        // Op 0: Source
        match &ops[0] {
            Op::Source {
                uri,
                format,
                row_type: _,
            } => {
                assert_eq!(uri.as_str(), "examples/users.csv");
                assert_eq!(*format, SourceFormat::Csv);
            }
            other => panic!("expected Source at index 0, got {other:?}"),
        }

        // Op 1: Extend(iri = 'https://example.org/user/' || users.id)
        match &ops[1] {
            Op::Extend { input, field, expr } => {
                assert_eq!(*input, 0);
                assert_eq!(field.as_str(), "iri");
                match expr {
                    ExprLowered::Concat(l, r) => match (l.as_ref(), r.as_ref()) {
                        (ExprLowered::LitString(lit), ExprLowered::ColRef { source, column }) => {
                            assert_eq!(lit.as_str(), "https://example.org/user/");
                            assert_eq!(source.as_str(), "users");
                            assert_eq!(column.as_str(), "id");
                        }
                        (lo, ro) => {
                            panic!("expected Concat(LitString, ColRef), got Concat({lo:?}, {ro:?})")
                        }
                    },
                    other => panic!("expected Concat for iri expr, got {other:?}"),
                }
            }
            other => panic!("expected Extend at index 1, got {other:?}"),
        }

        // Op 2: TripleEmit(subject_col=iri, predicate=ex:name, object_col=name)
        match &ops[2] {
            Op::TripleEmit {
                input,
                subject_col,
                predicate,
                object_col,
            } => {
                assert_eq!(*input, 1);
                assert_eq!(subject_col.as_str(), "iri");
                assert_eq!(predicate.as_str(), "https://example.org/name");
                assert_eq!(object_col.as_str(), "name");
            }
            other => panic!("expected TripleEmit at index 2, got {other:?}"),
        }

        // Op 3: Sink(GraphAr)
        match &ops[3] {
            Op::Sink { input, sink } => {
                assert_eq!(*input, 2);
                assert_eq!(*sink, SinkRef::GraphAr);
            }
            other => panic!("expected Sink at index 3, got {other:?}"),
        }
    }

    #[test]
    fn lower_to_mir_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("hello has one mapping");
        let a = lower_to_mir(&db, mapping);
        let b = lower_to_mir(&db, mapping);
        assert_eq!(a, b);
    }

    #[test]
    fn template_lowering_handles_trailing_literal() {
        // `${.id}/profile` → Concat(ColRef(users.id), LitString("/profile"))
        let raw = "`${.id}/profile`";
        let binding = SmolStr::new_static("users");
        let lowered = lower_iri_template_phase1(raw, &binding);
        match lowered {
            ExprLowered::Concat(l, r) => {
                assert!(matches!(l.as_ref(), ExprLowered::ColRef { .. }));
                assert!(
                    matches!(r.as_ref(), ExprLowered::LitString(s) if s.as_str() == "/profile")
                );
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }
}
