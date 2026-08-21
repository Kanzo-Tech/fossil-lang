//! Type ADT.
//!
//! Every kind here is a kind a PROGRAM can have. There is no
//! checker-state kind: `Unknown(InferenceId)` was one, and «no type» has a
//! spelling already — `synth_ty` returns `Option<Ty>` and every caller reads
//! it. Worse, `Unknown` did the opposite of what it was for: `subtypes` has no
//! arm for it, so it fell through to `_ => false` and REFUSED every check it
//! reached, where its own comment asked it to stand aside and let the shape
//! answer.
//!
//! There is no count to quote here, and there used to be: this said «all 11
//! kinds», and three of them — `Optional`, `Fn` and `Unknown` — were
//! constructed by nothing that a program could reach. `grammar.bnf` has no `T?`
//! and declares `FunctionDecl` absent (the user declares no functions), so
//! neither of the first two had a way into the language.
//!
//! Interning strategy:
//! - `Ty<'db>` itself is `#[salsa::interned]` so structural equality → pointer eq.
//! - `Record<'db>` is separately interned (many distinct field sets).
//! - All other kinds inline in `TyKind` directly.
//!
//! `'db` lifetime per Salsa 0.20+.

use fossil_base::ErrorGuaranteed;
// The primitive lattice is NOT the type system's to own: the schema contract, the
// descriptors and the checker all speak it, so it lives in the leaf they share
// (`fossil-graph-schema`) and is imported here like any other type. Salsa is fine
// with a foreign type: the `Update` derive falls back to `PartialEq` comparison
// for anything that is not `salsa::Update` itself.
use fossil_graph_schema::Primitive;
use smol_str::SmolStr;

/// Type pretty-printing.
///
/// The single source of truth for rendering Fossil types. It lives here and
/// not in `fossil-ide::hover`, where it was written, because the checker's
/// diagnostics render types too and the checker is below the IDE.
pub mod display;

/// Interned type handle. Two equal-shaped types share a single `Ty<'db>` id —
/// structural equality is pointer equality at the Salsa storage layer.
#[salsa::interned(debug)]
pub struct Ty<'db> {
    #[returns(ref)]
    pub kind: TyKind<'db>,
}

/// Every type kind the compiler can build.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyKind<'db> {
    /// Primitive types — the lattice [`fossil_graph_schema::Primitive`]
    /// declares, and the only one in the tree.
    Primitive(Primitive),
    /// `T*` — sequence / repeated.
    Seq(Ty<'db>),
    /// Tabular row type — interned separately for fast equality.
    Record(Record<'db>),
    /// An IRI value (RDF resource).
    Iri,
    /// An IRI template — backtick string with `${...}` placeholders.
    IriTemplate,
    /// Type-check failure taint. Carries [`ErrorGuaranteed`] directly (Phase 2
    /// promotion of a local taint-wrapper newtype).
    Error(ErrorGuaranteed),
}

/// One named field of a [`Record`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct RecordField<'db> {
    pub name: SmolStr,
    pub ty: Ty<'db>,
}

/// Record type — a row schema. Stored as `Vec<RecordField<'db>>` (Phase 1
/// pattern preserved — `IndexMap` doesn't impl Hash, Salsa needs Hash).
#[salsa::interned(debug)]
pub struct Record<'db> {
    #[returns(ref)]
    pub fields: Vec<RecordField<'db>>,
}

// `FnSig` stood here, interned, and it was the SECOND half of the same finding
// that removed `TyKind::Fn`. The checker reads `crate::stdlib::SigSpec`
// directly — `param.ty.to_ty(db)` in `synth_call` — so a materialised signature
// was a bridge with nobody on it: `RegistryEntry::signature` and
// `SigSpec::to_fn_sig` had one caller between them in the whole tree, and it
// was the test asserting that the bridge worked.
//
// A language with no `FunctionDecl` and no `LambdaExpr` cannot write a
// function-typed VALUE down, so nothing downstream can need one.

/// Shape identifier — newtype around a raw `u32`. Minted per-mapping by
/// [`crate::shapes::resolve_target_shape`] from the mapping's index, since a
/// mapping targets at most one shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ShapeId(pub u32);

impl ShapeId {
    /// The per-mapping id, and the one tests construct directly. It is called
    /// `placeholder` because it is not yet an interning of the shape's IRI —
    /// two mappings targeting one shape have two ids.
    #[must_use]
    pub const fn placeholder(raw: u32) -> Self {
        Self(raw)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    /// Type-level proof that `TyKind::Error(ErrorGuaranteed)` exists as a
    /// variant. `ErrorGuaranteed` cannot be constructed outside `fossil-base`
    /// (only via `delay_span_bug` / `bug` from inside a tracked query that
    /// has a `Diagnostic` sink), so we cannot build one here; the function
    /// body merely needs to match the variant. If `Ty::Error` is renamed or
    /// dropped this function stops compiling.
    const fn _ty_error_variant_exists(t: &TyKind<'_>) {
        if let TyKind::Error(_) = t { /* OK */ }
    }

    #[test]
    fn every_ty_kind_exists() {
        let db = db();
        let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let _: TyKind<'_> = TyKind::Primitive(Primitive::String);
        let _: TyKind<'_> = TyKind::Seq(int_ty);
        let rec = Record::new(&db, vec![]);
        let _: TyKind<'_> = TyKind::Record(rec);
        let _: TyKind<'_> = TyKind::Iri;
        let _: TyKind<'_> = TyKind::IriTemplate;
        // Reference the helper so the dead-code lint doesn't flag it.
        let _ = _ty_error_variant_exists as fn(&TyKind<'_>);
    }

    #[test]
    fn ty_interning_dedupes_structurally_equal_types() {
        let db = db();
        let a = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let b = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        assert_eq!(a, b); // salsa interning → same id

        let seq_int_a = Ty::new(&db, TyKind::Seq(a));
        let seq_int_b = Ty::new(&db, TyKind::Seq(b));
        assert_eq!(seq_int_a, seq_int_b);

        let seq_seq_a = Ty::new(&db, TyKind::Seq(seq_int_a));
        let seq_seq_b = Ty::new(&db, TyKind::Seq(seq_int_b));
        assert_eq!(seq_seq_a, seq_seq_b, "nesting interns structurally too");

        // Different shape → different id.
        assert_ne!(seq_int_a, seq_seq_a);
    }

    #[test]
    fn record_interning_works_for_multiple_field_sets() {
        let db = db();
        let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let str_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
        let r1 = Record::new(
            &db,
            vec![
                RecordField {
                    name: SmolStr::from("id"),
                    ty: int_ty,
                },
                RecordField {
                    name: SmolStr::from("name"),
                    ty: str_ty,
                },
            ],
        );
        let r2 = Record::new(
            &db,
            vec![
                RecordField {
                    name: SmolStr::from("id"),
                    ty: int_ty,
                },
                RecordField {
                    name: SmolStr::from("name"),
                    ty: str_ty,
                },
            ],
        );
        assert_eq!(r1, r2); // structural eq → same interned id
    }
}
