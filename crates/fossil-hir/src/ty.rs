//! Phase 1 minimal [`Ty`] ADT — 5 of the 11 kinds described in `type-system.md`.
//!
//! Carries `'db` lifetime per Salsa 0.20+ requirement on interned types.
//! Phase 2 (CORE-03) expands to all 11 kinds (`Optional`, `Seq`, `Shape`, `Fn`,
//! `TripleTerm`, plus the rest of the primitives). The public type names are
//! locked for the Phase 2-9 contract; only the variant set grows.
//!
//! # Design notes (Phase 1 deviations from the plan)
//!
//! - [`Record`] is a `#[salsa::interned]` struct rather than a plain
//!   `derive(salsa::Update)` struct. The `derive(Update)` form requires
//!   `'static` types; interning sidesteps that and gives us cheap structural
//!   equality (Phase 2 forward propagation from CSVW will hammer this path).
//! - The record fields are stored as a `Vec<(SmolStr, Ty<'db>)>` (insertion-
//!   ordered, like the original `IndexMap` plan but `Hash`-able as required by
//!   Salsa's tracked-struct invariant). The accessor returns `&[..]` so callers
//!   that need map semantics can build their own lookup.

use smol_str::SmolStr;

use crate::check::ErrorMarker;

/// Interned type handle. Two equal-shaped types share a single `Ty<'db>` id,
/// so structural equality is pointer equality at the Salsa storage layer.
#[salsa::interned(debug)]
pub struct Ty<'db> {
    #[returns(ref)]
    pub kind: TyKind<'db>,
}

/// The 5 Phase 1 type kinds. Phase 2 (CORE-03) adds:
/// `Optional(Ty<'db>)`, `Seq(Ty<'db>)`, `Shape(ShapeId)`,
/// `Fn(Vec<Ty<'db>>, Ty<'db>)`, `TripleTerm`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TyKind<'db> {
    /// `Primitive::String` ↔ xsd:string. `Primitive::Integer` ↔ xsd:integer.
    Primitive(Primitive),
    /// An IRI value (RDF resource).
    Iri,
    /// An IRI template — backtick string with `${...}` placeholders.
    /// Phase 1 codegen parses the template at SQL-emission time. Phase 4 lifts
    /// template parsing into a real expression tree.
    IriTemplate,
    /// A row of named fields (e.g. a CSV row, a JSON object, an `XPath` result).
    Record(Record<'db>),
    /// Type-check failure marker. Carries an [`ErrorMarker`] taint so downstream
    /// queries short-circuit without emitting cascading errors (the rustc
    /// `ErrorGuaranteed` pattern).
    Error(ErrorMarker),
}

/// Phase 1 primitive types. Phase 2 (CORE-03) adds `Float`, `Bool`, `Date`,
/// `DateTime`, `Time`, `gYear`, `AnyURI`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Primitive {
    String,
    Integer,
}

/// One named field of a [`Record`] — a `(name, type)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct RecordField<'db> {
    pub name: SmolStr,
    pub ty: Ty<'db>,
}

/// Record type — a row schema. The fields are insertion-ordered, which matters
/// for stable codegen output.
///
/// Stored as `Vec<RecordField<'db>>` (rather than the originally-planned
/// `IndexMap<SmolStr, Ty<'db>>`) because Salsa's tracked/interned struct
/// fields require [`std::hash::Hash`], and the workspace `indexmap = "2"`
/// pin does not implement Hash on `IndexMap`. A named-struct field type was
/// chosen over a `(SmolStr, Ty<'db>)` tuple because the `salsa::Update` derive
/// for tuple containers does not currently propagate the `'db` lifetime
/// through to the generated bounds.
#[salsa::interned(debug)]
pub struct Record<'db> {
    #[returns(ref)]
    pub fields: Vec<RecordField<'db>>,
}
