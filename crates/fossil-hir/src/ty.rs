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
    /// A relation: the named rows a `from` clause or a pipeline puts in scope.
    ///
    /// It was not a type at all. `RowScope` lived in `crate::infer`, beside the
    /// type system, so `seq.where` — a catalogue row on a `Relation` receiver —
    /// had the signature `p("rows", S::String)` under a comment reading
    /// «higher-order arguments collapse to scalar placeholders», and the only
    /// checking a pipeline stage got was a hand-written walk for the column
    /// NAMES its predicate mentions. `Row.celsius > "abc"` passed clean two
    /// lines above a call that was refused for the same mismatch.
    ///
    /// A list of named rows and not one flat record, because a join keeps both
    /// sides addressable — see [`Rows`].
    Relation(Rows<'db>),
    /// A reference to a node of one of the named shapes — `@shop:Person` in a
    /// shape document, `Person(User.email)` in a program.
    ///
    /// **It was `Iri`, and being RDF vocabulary is what left it untyped.** An
    /// IRI is an IRI, so every reference had one type and the check a graph
    /// language exists to make did not happen: `shop.shex` declares
    /// `shop:buyer @shop:Person`, a program wrote `buyer = Order(Purchase.id)`,
    /// and the compiler said `ok — no errors`. Both producers held the answer
    /// and dropped it — `expected_value_ty` knows the constraint's `targets`,
    /// `synth_edge` knows the shape it is minting an identity for.
    ///
    /// That an identity is SPELLED as an IRI is the shape decoder's business
    /// and the materialiser's. The core concept is the reference.
    ///
    /// A SET and not one name, because `targets` is a `Vec`: `@<A> OR @<B>`
    /// (`ShEx`) and `sh:or` (SHACL) are legal and emit one edge type per
    /// destination. Subtyping is set inclusion — a reference to `A` satisfies a
    /// slot that accepts `A` or `B`, which is how a member type is assignable
    /// to a union in `GraphQL` and how `sh:or` reads. Canonicalised by
    /// [`Ty::reference`] so two spellings of one set intern to one `Ty`.
    Ref(Vec<SmolStr>),
    /// Type-check failure taint. Carries [`ErrorGuaranteed`] directly (Phase 2
    /// promotion of a local taint-wrapper newtype).
    Error(ErrorGuaranteed),
}

impl<'db> Ty<'db> {
    /// A reference to a node of any of `shapes`, canonicalised.
    ///
    /// Sorted and deduplicated so that `@<A> OR @<B>` and `@<B> OR @<A>` are one
    /// interned `Ty` — the set is the type, and the order a document happened to
    /// write it in is not part of it.
    #[must_use]
    pub fn reference(
        db: &'db dyn salsa::Database,
        shapes: impl IntoIterator<Item = SmolStr>,
    ) -> Self {
        let mut names: Vec<SmolStr> = shapes.into_iter().collect();
        names.sort_unstable();
        names.dedup();
        Self::new(db, TyKind::Ref(names))
    }

    /// The shapes this type references, or `None` when it is not a reference.
    #[must_use]
    pub fn referenced_shapes(self, db: &'db dyn salsa::Database) -> Option<&'db [SmolStr]> {
        match self.kind(db) {
            TyKind::Ref(names) => Some(names),
            _ => None,
        }
    }
}

/// The rows a relation makes addressable, each under the name of the BINDING
/// that introduced it.
///
/// This is the consequence of names `grammar.bnf` spells out under
/// `SourceDef`: *«a mapping body writes `User.name` and never
/// `Adults.name`, even when it draws `from Adults`»*. A binding ties the type
/// and the relation together, so a relation DERIVED from `User` — by `where`, by
/// `select`, by standing on the left of a `join` — keeps handing back rows that
/// are addressed as `User`. The derived name (`Adults`, `Reachable`, `Joined`)
/// names the relation and never a row.
///
/// Which is why this is a LIST and not one record. A join brings a second
/// binding into the same relation, and its columns stay under their own name:
/// `Purchase.amount` and `User.email` are two rows of one relation, and two
/// columns called `id` — one per side — are two distinct entries here even
/// though the flattened [`Self::flat`] record can only find the first. Ruling 17
/// of `SURFACE-PLAN.md` deleted the collision rule on the promise that
/// qualification would do that work; this is where it does it.
///
/// The row is an `Option` because a binding whose source declares no schema is
/// still a row a body may name: `hello.fossil` addresses columns of a source
/// with no descriptor at all. So membership (does this relation have a row
/// called `Contact`?) and typing (what is `Contact.email`?) are two different
/// questions, and only the first has an answer for every program.
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct Rows<'db> {
    rows: Vec<NamedRow<'db>>,
}

/// One row of a relation, under the binding name that introduced it.
///
/// A named struct and not a `(SmolStr, Option<Ty>)`, for the reason
/// [`crate::provenance::ExprTypeEntry`] already records: tuples do not
/// auto-implement `salsa::Update`, and this has to, because it rides inside a
/// [`TyKind`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct NamedRow<'db> {
    /// The binding a body addresses these columns by — `User`, never `Adults`.
    pub binding: SmolStr,
    /// The row itself, or `None` when the source declares no schema.
    pub row: Option<Ty<'db>>,
}

impl<'db> Rows<'db> {
    /// A relation over rows already built — what the row algebra hands back
    /// after a `select` has narrowed each one.
    #[must_use]
    pub(crate) const fn of(rows: Vec<NamedRow<'db>>) -> Self {
        Self { rows }
    }

    /// Every row, in written order.
    pub fn iter(&self) -> impl Iterator<Item = &NamedRow<'db>> {
        self.rows.iter()
    }

    /// The scope of a binding that reads a file: itself, and nothing else.
    #[must_use]
    pub fn one(binding: &str, row: Option<Ty<'db>>) -> Self {
        Self {
            rows: vec![NamedRow {
                binding: SmolStr::from(binding),
                row,
            }],
        }
    }

    /// The binding names this relation makes addressable, left to right.
    pub fn bindings(&self) -> impl Iterator<Item = &SmolStr> {
        self.rows.iter().map(|r| &r.binding)
    }

    /// Is there a row under this name — the question the qualified-reference
    /// diagnostic asks. TRUE with an untyped row; absence is not "no schema".
    #[must_use]
    pub fn has(&self, binding: &str) -> bool {
        self.rows.iter().any(|r| r.binding == binding)
    }

    /// The row a binding contributes. `None` both when the name is not in scope
    /// and when it is but its source declares no schema — ask [`Self::has`]
    /// first, because those two are different answers.
    #[must_use]
    pub fn row_of(&self, binding: &str) -> Option<Ty<'db>> {
        self.rows
            .iter()
            .find(|r| r.binding == binding)
            .and_then(|r| r.row)
    }

    /// Every column of every row, left to right, as one flat `Record` — what
    /// the checker resolves a BARE name against and what the row algebra prints
    /// in its refusals.
    ///
    /// `None` when any row in the scope is untyped: a record missing one side's
    /// columns would answer "unknown column" for a column that exists.
    #[must_use]
    pub fn flat(&self, db: &'db dyn salsa::Database) -> Option<Ty<'db>> {
        let fields = self.fields(db)?;
        Some(Ty::new(db, TyKind::Record(Record::new(db, fields))))
    }

    /// [`Self::flat`]'s fields, before they are interned.
    pub(crate) fn fields(&self, db: &'db dyn salsa::Database) -> Option<Vec<RecordField<'db>>> {
        let mut out = Vec::new();
        for r in &self.rows {
            out.extend(record_fields(db, r.row?)?);
        }
        Some(out)
    }

    /// The columns of ONE row, for a per-binding message.
    pub(crate) fn fields_of(
        &self,
        db: &'db dyn salsa::Database,
        binding: &str,
    ) -> Option<Vec<RecordField<'db>>> {
        record_fields(db, self.row_of(binding)?)
    }

    /// Both sides of a join, in written order.
    pub(crate) fn concat(mut self, other: Self) -> Self {
        self.rows.extend(other.rows);
        self
    }

    /// `Node as Other` — the right side of a self-join under its second name.
    ///
    /// The whole right scope collapses to one row, because the alias is one
    /// name: joining a multi-binding relation under an alias makes its columns
    /// reachable through the alias and through nothing else.
    pub(crate) fn rename_to(self, db: &'db dyn salsa::Database, alias: &SmolStr) -> Self {
        let row = self.flat(db);
        Self {
            rows: vec![NamedRow {
                binding: alias.clone(),
                row,
            }],
        }
    }
}

/// The columns of one row, or `None` when it is not a record.
///
/// The one spelling: `crate::infer` had a copy, and the two answered the same
/// question about the same type.
pub(crate) fn record_fields<'db>(
    db: &'db dyn salsa::Database,
    row: Ty<'db>,
) -> Option<Vec<RecordField<'db>>> {
    match row.kind(db) {
        TyKind::Record(rec) => Some(rec.fields(db).clone()),
        _ => None,
    }
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

// `ShapeId` stood here — a `u32` minted from a mapping's INDEX, so two mappings
// targeting one shape had two ids, which its own docblock recorded as the thing
// to fix. It is deleted rather than fixed: nothing read it. `TypeckOutput`
// carried a `target_shape` field with no reader in the workspace, and
// `BlamePos::ShapeProperty` carried it beside a `property` name into the one
// arm that matches the variant with `{ .. }`.
//
// What identifies a shape is its IRI, and that is what `TyKind::Ref` carries.

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
        let _: TyKind<'_> = TyKind::Ref(vec![SmolStr::new_static("https://example.org/Person")]);
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
