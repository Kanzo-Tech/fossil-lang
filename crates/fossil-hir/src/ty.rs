//! Type ADT.
//!
//! The surface kinds + 1 internal `Unknown(InferenceId)`
//! kind for bidirectional checker state. The
//! internal kind is never exposed in surface diagnostics; it appears only
//! during checking-in-flight.
//!
//! There is no count to quote here, and there used to be: this said «all 11
//! kinds», and two of them — `Optional` and `Fn` — were constructed by nothing
//! but the test that enumerated them. Both are gone. `grammar.bnf` has no `T?`
//! and declares `FunctionDecl` absent (the user declares no functions), so
//! neither had a way into the language.
//!
//! Interning strategy:
//! - `Ty<'db>` itself is `#[salsa::interned]` so structural equality → pointer eq.
//! - `Record<'db>` is separately interned (many distinct field sets).
//! - `FnSig<'db>` is separately interned. It is the STDLIB's signature type
//!   (`crate::stdlib::SigSpec::to_fn_sig`), not a `TyKind` any more.
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
    /// Internal inference-state placeholder. Used by bidirectional checker
    /// during synthesis-mode descent; never exposed in surface diagnostics.
    /// Phase 3 fills this in with real inference logic. Phase 2 ships the
    /// variant + cheap newtype.
    Unknown(InferenceId),
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

/// Function signature — `(τ₁, ..., τₙ) → τ_r`. Interned so many distinct
/// signatures (from the stdlib catalog) share storage.
///
/// NOT a [`TyKind`]. `TyKind::Fn(FnSig)` existed and nothing constructed it:
/// the checker reads `crate::stdlib`'s `SigSpec` directly in `synth_call` and
/// never needs a function-typed VALUE, because the language has no way to write
/// one down (`grammar.bnf`: no `FunctionDecl`, no `LambdaExpr`).
#[salsa::interned(debug)]
pub struct FnSig<'db> {
    #[returns(ref)]
    pub params: Vec<Ty<'db>>,
    pub return_ty: Ty<'db>,
}

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

/// Inference variable id — used internally by the bidirectional checker for
/// not-yet-resolved synthesis positions. Fresh per check run; never stable
/// across query invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct InferenceId(pub u32);

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
        let _: TyKind<'_> = TyKind::Unknown(InferenceId(0));
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

    #[test]
    fn fnsig_interning_dedupes_equal_signatures() {
        let db = db();
        let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let sig_a = FnSig::new(&db, vec![int_ty, int_ty], int_ty);
        let sig_b = FnSig::new(&db, vec![int_ty, int_ty], int_ty);
        assert_eq!(sig_a, sig_b);
    }
}
