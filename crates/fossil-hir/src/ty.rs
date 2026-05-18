//! Type ADT — Phase 2 (CORE-03) expansion to all 11 kinds.
//!
//! 10 surface kinds (type-system.md §2) + 1 internal `Unknown(InferenceId)`
//! kind for bidirectional checker state (per Phase 2 RESEARCH.md §Q4 — the
//! "11th kind" interpretation). The internal kind is never exposed in surface
//! diagnostics; it appears only during checking-in-flight.
//!
//! Interning strategy per RESEARCH.md §Q4:
//! - `Ty<'db>` itself is `#[salsa::interned]` so structural equality → pointer eq.
//! - `Record<'db>` is separately interned (many distinct field sets).
//! - `FnSig<'db>` is separately interned (many distinct function signatures).
//! - All other kinds inline in `TyKind` directly.
//!
//! `'db` lifetime per Salsa 0.20+ (ADR-0003 + RESEARCH.md §Q7).

use fossil_base::ErrorGuaranteed;
use smol_str::SmolStr;

/// Interned type handle. Two equal-shaped types share a single `Ty<'db>` id —
/// structural equality is pointer equality at the Salsa storage layer.
#[salsa::interned(debug)]
pub struct Ty<'db> {
    #[returns(ref)]
    pub kind: TyKind<'db>,
}

/// All 11 type kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyKind<'db> {
    /// Primitive types per type-system.md §2 line 41-42 (9 variants).
    Primitive(Primitive),
    /// `T?` — nullable wrapper, type-system.md §2.
    Optional(Ty<'db>),
    /// `T*` — sequence / repeated, type-system.md §2.
    Seq(Ty<'db>),
    /// Tabular row type — interned separately for fast equality.
    Record(Record<'db>),
    /// An IRI value (RDF resource).
    Iri,
    /// An IRI template — backtick string with `${...}` placeholders.
    IriTemplate,
    /// Satisfies ShEx shape S. [`ShapeId`] is a stub in Phase 2; Phase 3
    /// resolves it via `fossil-descriptors-output`.
    Shape(ShapeId),
    /// Function signature — interned separately for fast equality.
    Fn(FnSig<'db>),
    /// RDF 1.2 quoted triple-as-term, type-system.md §2 + §4.10.
    TripleTerm,
    /// Type-check failure taint. Carries [`ErrorGuaranteed`] directly (Phase 2
    /// promotion of Phase 1's `ErrorMarker` newtype — see ADR-0004 +
    /// RESEARCH.md §Q6).
    Error(ErrorGuaranteed),
    /// Internal inference-state placeholder. Used by bidirectional checker
    /// during synthesis-mode descent; never exposed in surface diagnostics.
    /// Phase 3 fills this in with real inference logic. Phase 2 ships the
    /// variant + cheap newtype.
    Unknown(InferenceId),
}

/// All 9 primitive types per type-system.md §2 line 41-42 (xsd-aligned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Primitive {
    /// `xsd:string`.
    String,
    /// `xsd:integer`.
    Integer,
    /// `xsd:float` / `xsd:double`.
    Float,
    /// `xsd:boolean`.
    Bool,
    /// `xsd:date`.
    Date,
    /// `xsd:dateTime`.
    DateTime,
    /// `xsd:time`.
    Time,
    /// `xsd:gYear` (capitalisation: `GYear` in Rust style; XSD spelling
    /// preserved in docs).
    GYear,
    /// `xsd:anyURI`.
    AnyURI,
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
/// signatures (from the function registry) share storage.
#[salsa::interned(debug)]
pub struct FnSig<'db> {
    #[returns(ref)]
    pub params: Vec<Ty<'db>>,
    pub return_ty: Ty<'db>,
}

/// Shape identifier — newtype around a raw `u32`. Resolved by
/// `fossil-descriptors-output` in Phase 3 (ShEx integration); Phase 2 ships
/// the variant + a `placeholder` constructor for test fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ShapeId(pub u32);

impl ShapeId {
    /// Placeholder shape id used by Phase 2 tests; Phase 3 will replace with
    /// real ShEx resolution via the `OutputDescriptor` trait.
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        fossil_base::FossilDb::new(system)
    }

    #[test]
    fn all_eleven_ty_kinds_exist() {
        let db = db();
        let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let _: TyKind<'_> = TyKind::Primitive(Primitive::String);
        let _: TyKind<'_> = TyKind::Optional(int_ty);
        let _: TyKind<'_> = TyKind::Seq(int_ty);
        let rec = Record::new(&db, vec![]);
        let _: TyKind<'_> = TyKind::Record(rec);
        let _: TyKind<'_> = TyKind::Iri;
        let _: TyKind<'_> = TyKind::IriTemplate;
        let _: TyKind<'_> = TyKind::Shape(ShapeId::placeholder(0));
        let sig = FnSig::new(&db, vec![int_ty], int_ty);
        let _: TyKind<'_> = TyKind::Fn(sig);
        let _: TyKind<'_> = TyKind::TripleTerm;
        // ErrorGuaranteed cannot be constructed outside fossil-base (only via
        // delay_span_bug / bug from inside a tracked query that has a Diagnostic
        // sink). The variant's existence is proven structurally by the match
        // arm below — if Ty::Error is renamed or dropped the test won't compile.
        fn _shape_only(t: TyKind<'_>) {
            match t {
                TyKind::Error(_) => {}
                _ => {}
            }
        }
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

        let opt_seq_a = Ty::new(&db, TyKind::Optional(seq_int_a));
        let opt_seq_b = Ty::new(&db, TyKind::Optional(seq_int_b));
        assert_eq!(opt_seq_a, opt_seq_b);

        // Different shape → different id.
        let opt_int = Ty::new(&db, TyKind::Optional(a));
        assert_ne!(opt_int, opt_seq_a);
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

    #[test]
    fn all_nine_primitive_variants_exist() {
        // Compile-time enumeration check: every variant per
        // type-system.md §2 line 41-42 must be present.
        let _vs = [
            Primitive::String,
            Primitive::Integer,
            Primitive::Float,
            Primitive::Bool,
            Primitive::Date,
            Primitive::DateTime,
            Primitive::Time,
            Primitive::GYear,
            Primitive::AnyURI,
        ];
        assert_eq!(_vs.len(), 9);
    }
}
