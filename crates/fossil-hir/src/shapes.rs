//! Target-shape resolution.
//!
//! Maps a mapping's declared shape IRI to a Phase-3-internal [`ResolvedShape`]
//! carrying the per-predicate constraint table, converted from the
//! format-neutral [`fossil_graph_schema::Shape`].
//!
//! # The compiler is handed a value, not a parser
//!
//! This module used to name `ShExDescriptor`, `ShapeBinding`, `IriS` and
//! `shex_ast::ShapeExpr`, which is how `fossil-hir` linked a schema language.
//! It now names [`fossil_graph_schema::shapes`]'s vocabulary and nothing else:
//! a host installs a [`fossil_base::Provider`] row, its decode lowers its own
//! syntax into [`fossil_graph_schema::OutputShapes`], and the checker never
//! learns which language the document was written in. Same cut `0e6898d` made
//! for `fossil-mir`.
//!
//! # The document the program names
//!
//! [`resolve_target_shape`] reads the shape document the PROGRAM brings in with
//! `type { … } := io.shex("…")`. It used to take the descriptor as a borrowed
//! ARGUMENT threaded in by the host; the in-query caller had nothing to thread
//! and passed `ACCEPT_ALL_DEFAULT`, so backward checking was off for every
//! program compiled through the checker — an axis with exactly one value.
//!
//! It then read the bytes with `System::read_file`, which registers no Salsa
//! dependency: editing the document re-ran nothing, and in an LSP that is a
//! diagnostic that never clears. The document is now a [`fossil_base::SourceFile`]
//! **input** found through [`fossil_base::file_at`] and decoded by the tracked
//! [`crate::shape_documents::shape_document`], so an edit to the `.shex` invalidates every
//! mapping checked against it.
//!
//! Naming no document is an ERROR: a property key is a bare name whose meaning
//! is the last segment of a predicate IRI **the document declares**, so a
//! program with no document cannot write a single property. `lower_to_hir`
//! reports it, at the header whose name bound nothing.
//!
//! # Every failure is reported at the BINDING, and none of them here
//!
//! Four more variants stood beside it — a document nothing registered, one no
//! decoder reads, one that did not parse, one that does not declare the shape
//! the mapping targets — each written so the commonest mistake, a misspelt
//! shape name, would produce a message instead of a silent `None`. They are
//! gone, and the reason is the ORDER, not the fixtures.
//!
//! A mapping header names a bare LOCAL name. [`crate::def_map`] binds those
//! names POSITIONALLY, against this module's own [`decoded_document`], before
//! anything asks a mapping for its target shape. So a document that cannot
//! answer fails THERE: the binding takes a
//! [`ShapeBindError`](crate::def_map::ShapeBindError), `lookup_type` answers
//! `None`, [`crate::lower`] reports it by cause (`unbound_shape_problem`, which
//! carries the did-you-mean over the names the program bound) and leaves
//! `shape_iri` empty — and an empty shape IRI reads here as «no shape clause».
//! `Undeclared` was doubly unreachable on top of that: a positional binding
//! hands over the Nth shape the document DECLARES, so the IRI can only be one
//! the document declares.
//!
//! Every way a program can fail to bring a shape in was driven through
//! [`resolve_target_shape`] to settle it — the corpus is
//! `a_document_that_cannot_answer_leaves_the_mapping_with_no_shape_clause`, one
//! row per way, plus the no-`type`-line case beside it — and every one answered
//! `None`. Not one reached a construction site.
//!
//! `None` therefore means two things that used to be one, and the second is
//! the residue: a mapping with NO SHAPE CLAUSE, and a mapping whose shape name
//! bound nothing and has already been told so.
//!
//! # Why the failure is returned and not emitted here
//!
//! A `salsa` accumulator panics outside an active tracked function
//! (`Accumulator::push`: "cannot accumulate values outside of an active tracked
//! function"), and this is a plain-Rust helper that IDE features call directly.
//! So it reports by value, exactly as [`crate::def_map`]'s
//! [`ShapeBindError`](crate::def_map::ShapeBindError) already does for the
//! source side, and [`crate::check`] — which runs inside `typecheck_mapping` —
//! turns it into a diagnostic.

use fossil_graph_schema::{
    Occurs, OutputShapes, Primitive, Problem, PropertyConstraint, Rejection, Shape, Span,
};
use smol_str::SmolStr;

use crate::def_map::MappingLoc;
use crate::ty::{Ty, TyKind};

/// One predicate constraint converted from a format-neutral
/// [`PropertyConstraint`] into Fossil type space.
///
/// It is a separate type from [`PropertyConstraint`] for one reason: it carries
/// an interned [`Ty<'db>`], which the shape vocabulary cannot — that crate has
/// no database and no lifetime. The conversion is where the two decisions below
/// are made, and making them once here is the point of the type.
#[derive(Debug, Clone)]
pub struct ShapeConstraint<'db> {
    /// The predicate IRI (fully-resolved string form).
    pub predicate: SmolStr,
    /// The type a value must have to satisfy this constraint, or `None` when
    /// **the document did not narrow the value type** — see
    /// [`expected_value_ty`], which is where that is decided.
    pub value_ty: Option<Ty<'db>>,
    /// How many values the property may carry.
    pub occurs: Occurs,
    /// Where the DOCUMENT declares this predicate — see
    /// [`fossil_graph_schema::PropertyConstraint::span`]. Paired with
    /// [`ResolvedShape::document`], it is a label in the `.shex`.
    pub span: Option<Span>,
}

/// A mapping's resolved target shape — Phase-3-internal.
#[derive(Debug, Clone)]
pub struct ResolvedShape<'db> {
    /// Per-predicate constraint table.
    pub constraints: Vec<ShapeConstraint<'db>>,
    /// Where the document declares the shape —
    /// [`fossil_graph_schema::Shape::span`].
    pub span: Option<Span>,
    /// The document that declared this shape, as the PROGRAM named it
    /// (`io.shex("shape.shex")` → `shape.shex`).
    ///
    /// One per shape and not one per constraint: a shape is declared in exactly
    /// one document — `shape_binding_for` resolves which, and getting that
    /// wrong is what `docs/programs/multi-document` exists to catch — so a
    /// copy per predicate would be the same string N times with N chances to
    /// disagree.
    pub document: SmolStr,
    /// What the decoder could not lower, filtered to this shape;
    /// [`crate::check::Checker::surface_shape_lowering_errors`] surfaces these
    /// as diagnostics on the consuming mapping.
    pub rejections: Vec<Rejection>,
}

impl<'db> ResolvedShape<'db> {
    /// Build a [`ResolvedShape`] from a decoded [`Shape`].
    ///
    /// Plain-Rust helper. What identifies the shape is its IRI, and the checker
    /// reads that off the mapping; this table is the CONSTRAINTS, which is what
    /// it reads a resolved shape for.
    #[must_use]
    pub fn from_shape(
        db: &'db dyn fossil_base::Db,
        shape: &Shape,
        rejections: Vec<Rejection>,
        document: SmolStr,
    ) -> Self {
        let constraints = shape
            .properties
            .iter()
            .map(|c| ShapeConstraint {
                predicate: SmolStr::from(c.predicate.as_str()),
                value_ty: expected_value_ty(db, c),
                occurs: c.occurs,
                span: c.span,
            })
            .collect();
        Self {
            constraints,
            span: shape.span,
            document,
            rejections,
        }
    }

    /// The shape's predicates by the SHORT NAME a program writes, and the
    /// collisions that make some of them unwritable.
    ///
    /// The short name is [`fossil_graph_schema::short_name`] — the `@rename`
    /// addressed to the predicate, else the last segment of its IRI. That is
    /// the same function `OutputShapes::to_graph_schema` emits the column with,
    /// which is what stops a renamed predicate being written under one name and
    /// emitted under another.
    ///
    /// **A collision is an error naming both IRIs, and never a numeric suffix.**
    /// The evidence is the authors' own: FSharp.Data's PLDI-2016 paper declares
    /// in §6.5 that its algorithm avoids anything where «a small change in the
    /// sample causes a large change in the provided types», and then does not
    /// apply that criterion to names — its collision scheme is `PascalCase2`,
    /// its singulariser produced `Purchasis` and `Sourcis`, and a minor version
    /// renamed members and broke a program in production. The repair here is
    /// the author's and it is all constants: `@rename` above the type binding.
    ///
    /// Both halves come back from one walk because the caller needs both: the
    /// table to resolve what the body wrote, and the collisions to say why a
    /// name it could not resolve is unwritable rather than unknown.
    ///
    /// `renames` is the repair, in the order the program wrote it: each entry is
    /// `(predicate IRI, the name to use instead)`, read off the `@rename`
    /// attributes above the `type` binding that introduced this shape's name
    /// ([`crate::def_map::TypeEntry::renames`]). A predicate with a rename takes
    /// it INSTEAD of its last segment, and the collision walk runs AFTER the
    /// substitution — which is the only order in which the repair repairs
    /// anything.
    #[must_use]
    pub fn short_names(
        &self,
        renames: &[(SmolStr, SmolStr)],
    ) -> (Vec<(SmolStr, SmolStr)>, Vec<NameCollision>) {
        let mut table: Vec<(SmolStr, SmolStr)> = Vec::with_capacity(self.constraints.len());
        let mut collisions: Vec<NameCollision> = Vec::new();
        for c in &self.constraints {
            let short = SmolStr::from(fossil_graph_schema::short_name(
                c.predicate.as_str(),
                renames,
            ));
            if let Some((_, first)) = table.iter().find(|(n, _)| *n == short) {
                collisions.push(NameCollision {
                    name: short,
                    first: first.clone(),
                    second: c.predicate.clone(),
                });
            } else {
                table.push((short, c.predicate.clone()));
            }
        }
        (table, collisions)
    }

    /// Find the constraint matching a predicate IRI, if any.
    #[must_use]
    pub fn constraint_for(&self, predicate_iri: &str) -> Option<&ShapeConstraint<'db>> {
        self.constraints
            .iter()
            .find(|c| c.predicate.as_str() == predicate_iri)
    }
}

/// The type a value must have to satisfy `c`, or `None` when the document did
/// not narrow it.
///
/// Three cases, and the third one used to be the opposite of what it says:
///
/// 1. **A narrowed literal** (`datatype: Some(p)`) expects that primitive.
/// 2. **An edge** (`targets` non-empty) expects a [`TyKind::Ref`] to the shapes
///    the constraint names: the value is a reference to another node, and
///    naming WHICH is what stops every reference being one type.
/// 3. **Anything else** — the document mentioned the predicate and said nothing
///    about its value — expects NOTHING. The checker enforces the cardinality
///    and leaves the type alone.
///
/// Case 3 is the one that reads backwards. A missing value type resolved to the
/// NARROWEST type in the lattice while this module's own field doc said `None`
/// meant "any value", the WIDEST. Both could not be right: a shape that
/// declines to narrow the value must not reject a `String`, or `name = .name`
/// fails against `ex:name .` — a constraint that constrains nothing.
/// `PropertyConstraint::datatype`'s doc in `fossil-graph-schema` records the
/// contradiction and asks whoever rewrites this to choose deliberately; this is
/// the choice. A shape that does not narrow the value does not narrow it.
fn expected_value_ty<'db>(db: &'db dyn fossil_base::Db, c: &PropertyConstraint) -> Option<Ty<'db>> {
    if let Some(p) = c.datatype {
        return Some(Ty::new(db, TyKind::Primitive(p)));
    }
    if c.targets.is_empty() {
        None
    } else {
        // The constraint's destinations ARE the type. This used to answer
        // `TyKind::Iri` — «some IRI» — which made every reference in the
        // language one type, so an edge to the wrong shape type-checked.
        Some(Ty::reference(db, c.targets.iter().map(SmolStr::from)))
    }
}

/// Two predicates of one shape whose last IRI segments coincide.
///
/// Both IRIs are carried because the diagnostic names both — a message that
/// says only «`name` is ambiguous» leaves the author guessing which vocabulary
/// brought the second one in, and the repair has to quote an IRI exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameCollision {
    /// The short name the two predicates share.
    pub name: SmolStr,
    /// The predicate the shape declares first.
    pub first: SmolStr,
    /// The one that collides with it.
    pub second: SmolStr,
}

/// A name to suggest for a colliding predicate — `foaf_name` for
/// `http://xmlns.com/foaf/0.1/name`.
///
/// # This is a SUGGESTION, and that distinction is the whole point
///
/// The compiler is banned from **deriving** a name: FSharp.Data's numeric
/// suffix (`PascalCase2`) and its singulariser renamed members across a minor
/// version and broke a program in production, and its own PLDI-2016 §6.5
/// criterion — nothing where *«a small change in the sample causes a large
/// change in the provided types»* — is the evidence. What the ban forbids is a
/// name the author never sees becoming the name they depend on.
///
/// A suggestion is the opposite of that. The compiler REFUSES, and the repair
/// is `@rename`, which the author writes into the program as a constant. So
/// this may be wrong, and being wrong costs an edit rather than a silent
/// rename. What it may NOT be is unusable: the diagnostic recommends a line
/// that has to parse and has to repair, which is what
/// `the_recommended_rename_parses_and_repairs_the_collision` pins.
///
/// # The rule
///
/// The vocabulary that brought the predicate in, plus the short name — which is
/// exactly what the design writes by hand in its own example. The vocabulary
/// is the last token of the IRI **before** its last segment that is not one of
/// the tokens no vocabulary is told apart by: a scheme, `www`, a bare version
/// number, or a public suffix. `http://xmlns.com/foaf/0.1/name` yields
/// `[xmlns, com, foaf, 0, 1]`, and the survivors end at `foaf`.
///
/// No numeric suffix anywhere, in either the rule or its fallback: a predicate
/// whose IRI yields nothing gets `renamed_<short>`, which is ugly on purpose —
/// an author who sees it knows the compiler had nothing to go on.
#[must_use]
pub fn suggested_alias(predicate_iri: &str) -> SmolStr {
    /// Tokens that never distinguish one vocabulary from another. Written out
    /// rather than pattern-matched: a list you can read is a list you can argue
    /// with, and every entry here is a word that appears in IRIs from every
    /// vocabulary there is.
    const NOT_A_VOCABULARY: &[&str] = &[
        "http", "https", "www", "ns", "schema", "vocab", "voc", "com", "org", "net", "io", "edu",
        "gov", "co", "uk", "de", "fr", "es",
    ];

    let short = fossil_graph_schema::local_name(predicate_iri);
    // Everything before the last segment — the namespace the predicate sits in.
    let namespace = predicate_iri
        .strip_suffix(short)
        .unwrap_or(predicate_iri)
        .to_ascii_lowercase();
    let hint = namespace
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.chars().any(|c| c.is_ascii_alphabetic()))
        .filter(|t| !NOT_A_VOCABULARY.contains(t))
        .next_back();
    hint.map_or_else(
        || SmolStr::from(format!("renamed_{short}")),
        |h| SmolStr::from(format!("{h}_{short}")),
    )
}

/// The decoded document `path` names, read as the language `constructor` names.
///
/// The ONE place `fossil-hir` turns a path a program wrote into a shape
/// document. Both steps are Salsa-visible — [`fossil_base::file_at`] reads the
/// registry input (so registering the file later invalidates a reader that
/// missed) and [`crate::shape_documents::shape_document`] is tracked on the file's text
/// (so editing the document re-runs the checker). Neither was true of the
/// `System::read_file` this replaces.
///
/// # The row is chosen by NAME
///
/// `constructor` is what the program wrote — `io.shex`, `io.shacl` — and it
/// selects the row; the extension never does, or `io.shex("x.ttl")` and
/// `io.shacl("x.ttl")` would be the same program. The row is then asked two
/// questions it answers about itself, and each refusal comes back in the row's
/// own words: does it read TYPES at all, and does it accept this extension.
///
/// # Errors
///
/// The [`Problem`] a caller reports — `provider/*` when the row refuses,
/// `document/not-registered`, `document/unparseable`.
///
/// `constructor` is `None` only for a program that named a document without a
/// provider, and that is `provider/bare-document-path` — not a fallback. The
/// `schema =` argument of a source is a provider call like every other position
/// that names a document, so there is one rule and no corner left in it.
pub(crate) fn decoded_document(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    constructor: Option<&str>,
    path: &str,
) -> Result<OutputShapes, Problem> {
    use fossil_base::providers::{Capability, provider};

    let table = fossil_base::providers::installed(db);
    let ctor = constructor.ok_or_else(|| Problem::BareDocumentPath {
        document: path.to_string(),
    })?;
    let row = provider(table, ctor).ok_or_else(|| Problem::UnknownProvider {
        constructor: ctor.to_string(),
    })?;
    if !row.provides(Capability::ReadTypes) {
        return Err(Problem::WrongCapability {
            constructor: row.constructor(),
            capability: crate::refusals::capability(Capability::ReadTypes).to_string(),
        });
    }
    if !row.accepts(path) {
        return Err(Problem::WrongExtension {
            constructor: row.constructor(),
            document: path.to_string(),
        });
    }

    // The key, and the SAME function the hosts register under
    // (`crate::documents::registry_key`). A second spelling of it here is how
    // «the document nobody registered» and «the document registered under
    // another key» became the same message.
    let key = crate::documents::registry_key(db, file, path);
    let doc = fossil_base::file_at(db, &key).ok_or_else(|| Problem::NotRegistered {
        document: path.to_string(),
    })?;
    // An installed row that stopped reading types between the check and the
    // call: not reachable from a program.
    let shapes =
        crate::shape_documents::shape_document(db, doc, row.name).ok_or_else(|| Problem::Bug {
            what: format!("no installed provider reads `{path}`"),
        })?;
    malformed_cause(&shapes).map_or(Ok(shapes), |cause| {
        Err(Problem::Unparseable {
            document: path.to_string(),
            reason: Some(cause.to_string()),
        })
    })
}

/// The message of the document's top-level failure, if it has one.
///
/// [`Rejection::Malformed`] is the only rejection about the DOCUMENT — the
/// other three are about one shape inside it, and a document that carries them
/// still declares shapes worth reading.
fn malformed_cause(shapes: &OutputShapes) -> Option<SmolStr> {
    shapes.rejections().iter().find_map(|r| match r {
        Rejection::Malformed(m) => Some(SmolStr::from(m.as_str())),
        _ => None,
    })
}

/// Peel `Seq` wrappers to the inner [`Primitive`], if any — the datatype
/// carried on a vertex property column.
///
/// It peeled `Optional` too, and that variant is gone: nothing but a test ever
/// built one.
#[must_use]
pub fn inner_primitive<'db>(db: &'db dyn fossil_base::Db, ty: Ty<'db>) -> Option<Primitive> {
    match ty.kind(db) {
        TyKind::Primitive(p) => Some(*p),
        TyKind::Seq(inner) => inner_primitive(db, *inner),
        _ => None,
    }
}

/// Resolve a mapping's target shape against the document the PROGRAM names.
///
/// `None` means the mapping has nothing to check against. Every way a named
/// document can fail to produce a shape is reported at the `type` binding, by
/// `crate::lower::unbound_shape_problem`, and leaves the mapping's shape IRI
/// empty, so nothing is left to report here.
pub fn resolve_target_shape<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<ResolvedShape<'db>> {
    // The mapping's fully-resolved target shape IRI (prefix already expanded by
    // `lower_to_hir`). A mapping with no shape clause yields no resolution.
    let file = mapping.file(db);
    let hir = crate::lower::lower_to_hir(db, file);
    let shape_iri = hir.mapping(db, mapping.index(db))?.shape_iri.clone();
    if shape_iri.is_empty() {
        return None;
    }

    // The document that declared THIS shape — the binding whose `type { … } :=
    // io.shex("…")` introduced it, not the first document the file happens to
    // name. A program with two documents resolved its second mapping against the
    // wrong one until `shape_binding_for` existed; the fixture is
    // `docs/programs/multi-document`. A program that names none has no
    // output contract.
    let dm = crate::def_map::def_map(db, file);
    let (constructor, document) = dm.shape_binding_for(db, shape_iri.as_str())?;

    // This is the SECOND decode of the same document, and the first one has
    // already decided everything: `shape_iri` is non-empty only because
    // `def_map` decoded this document, positionally, and took a shape out of
    // it. So a failure here would have failed there — where it is reported, by
    // cause, with the `io.shex("…")` node's own span — and a shape the lookup
    // misses cannot exist, because the IRI came out of this document's
    // declaration list.
    let shapes = decoded_document(db, file, constructor.as_deref(), document.as_str()).ok()?;
    let shape = shapes.lookup(shape_iri.as_str())?;

    // Filter the document's rejections to this shape so the consuming mapping
    // surfaces only its own.
    let rejections: Vec<Rejection> = shapes
        .rejections()
        .iter()
        .filter(|r| rejection_targets(r, shape_iri.as_str()))
        .cloned()
        .collect();

    Some(ResolvedShape::from_shape(db, shape, rejections, document))
}

/// `true` iff a rejection belongs to the shape identified by `shape_iri`.
///
/// [`Rejection::Disjunction`] carries the offending shape IRI; the other
/// variants are conservatively associated with every shape (they describe
/// schema-wide problems the consuming mapping should still see). That is the
/// rule the `ShEx`-typed predecessor applied, kept verbatim — narrowing
/// [`Rejection::UnresolvedRef`] by its `in_shape` is now possible and is a
/// separate change with its own test.
fn rejection_targets(rejection: &Rejection, shape_iri: &str) -> bool {
    match rejection {
        Rejection::Disjunction { shape_iri: s, .. } => s == shape_iri,
        _ => true,
    }
}

// The `.fossil` sources below contain `{users.id}` interpolation holes and
// `type { … }` braces — LITERAL Fossil source, not Rust format-string args.
// Same allow, same reason, as the two `fossil-ide` integration tests.
#[allow(clippy::literal_string_with_formatting_args)]
#[cfg(test)]
mod tests {
    use super::*;

    use fossil_base::test_support::{PERSON_DOCUMENT, db_with_document, new_db};

    // --- the value type a constraint expects (defect 1) --------------------

    fn prop(datatype: Option<Primitive>, targets: &[&str]) -> PropertyConstraint {
        PropertyConstraint {
            predicate: "https://example.org/p".into(),
            datatype,
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            occurs: Occurs::ONE,
            // A test fixture, not a document: no text to point into.
            span: None,
        }
    }

    /// A constraint the document did not narrow expects NOTHING — it used to
    /// expect `Iri`, the narrowest type there is, so a shape that constrains
    /// nothing rejected a `String`.
    #[test]
    fn an_un_narrowed_constraint_expects_nothing() {
        let db = new_db();
        assert!(
            expected_value_ty(&db, &prop(None, &[])).is_none(),
            "`None` means the document did not narrow the value type, which is \
             the widest expectation and not the narrowest"
        );
    }

    /// The two cases that DO expect something: a narrowed literal expects its
    /// primitive, and an edge expects an IRI, because the only thing a
    /// reference to another node can be is an IRI. That second case is where
    /// the old `Iri` default was right, and it is now the only place it fires.
    #[test]
    fn a_narrowed_literal_and_an_edge_each_expect_their_own_type() {
        let db = new_db();
        let literal =
            expected_value_ty(&db, &prop(Some(Primitive::Integer), &[])).expect("narrowed");
        assert_eq!(literal.kind(&db), &TyKind::Primitive(Primitive::Integer));
        let edge =
            expected_value_ty(&db, &prop(None, &["https://example.org/City"])).expect("an edge");
        assert_eq!(
            edge,
            Ty::reference(
                &db,
                std::iter::once(SmolStr::new_static("https://example.org/City"))
            ),
            "an edge's value is a reference to the shape the constraint names, \
             and naming it is the whole point: `@<City>` and `@<Person>` were \
             one type while this said `Iri`"
        );
    }

    // --- resolve_target_shape reads the document the program names ---------

    /// A `.fossil` program whose single mapping targets the one shape
    /// `document` declares, and which NAMES that document. The document is the
    /// whole point: before this, a CSV-sourced program had nowhere to declare
    /// one, so the checker was handed `ACCEPT_ALL_DEFAULT` and checked nothing.
    ///
    /// The header names `Person`, a BARE NAME, and it is not looked up in the
    /// document: `type { Person } := …` binds POSITIONALLY, so `Person` is
    /// whatever the document declares first. The CURIE `ex:Person` that stood
    /// here read as a shape IRI in its own right, which is why the misspelling
    /// tests below could reach the document at all.
    fn src_naming(document: &str) -> String {
        format!(
            "type {{ Person }} := io.shex(\"{document}\")\n\
             users := io.csv(\"x.csv\")\n\
             User : Person from users\n    \
             @subject = \"http://example.org/u/{{users.id}}\"\n    \
             name = users.name\n"
        )
    }

    /// The same program with no `type` line — it names no document at all.
    const SRC_WITHOUT_DOCUMENT: &str = "\
users := io.csv(\"x.csv\")
User : Person from users
    @subject = \"http://example.org/u/{users.id}\"
    name = users.name
";

    fn first_mapping(db: &fossil_base::FossilDb, file: fossil_base::SourceFile) -> MappingLoc<'_> {
        crate::def_map::def_map(db, file).mappings(db)[0]
    }

    #[test]
    fn the_target_shape_comes_from_the_document_the_program_names() {
        let (db, file) =
            db_with_document(&src_naming("person.shex"), "person.shex", PERSON_DOCUMENT);
        let resolved = resolve_target_shape(&db, first_mapping(&db, file))
            .expect("the program names a document that declares the mapping's target shape");
        assert!(
            resolved.constraint_for("http://example.org/name").is_some(),
            "the resolved shape must carry the ex:name constraint"
        );
    }

    /// Naming no document is an error, and this test is that rule: a property
    /// key is the last segment of a predicate IRI that a shape declares, so a
    /// program with no shape document cannot write a property at all.
    ///
    /// A header names a bare name, resolved by `def_map`'s type bindings: a
    /// program with no `type` line binds no name, `lower_to_hir` reports it and
    /// leaves `shape_iri` empty, and this answers `None`.
    #[test]
    fn a_program_that_names_no_document_is_an_error_now() {
        let (db, file) = db_with_document(SRC_WITHOUT_DOCUMENT, "unused.shex", PERSON_DOCUMENT);
        assert!(
            resolve_target_shape(&db, first_mapping(&db, file)).is_none(),
            "the name bound nothing, so there is no output contract"
        );
        let diagnostics =
            crate::lower::lower_to_hir::accumulated::<fossil_base::Diagnostic>(&db, file);
        assert!(
            diagnostics.iter().any(|d| d.problem
                == fossil_graph_schema::Problem::UnknownShape {
                    shape: "Person".into(),
                    declared: Vec::new(),
                }
                && d.help
                    .as_deref()
                    .is_some_and(|h| h.contains("no shape names at all"))),
            "and the program is still told so, got: {:?}",
            diagnostics.iter().map(|d| d.message()).collect::<Vec<_>>(),
        );
    }

    /// The registry key is the document path joined onto the PROGRAM's
    /// directory, and a host has to compute the same string or every lookup
    /// misses. It is [`crate::documents::registry_key`] on both sides now —
    /// this is the half that lives where the LOOKUP happens, and
    /// `crate::documents`' own tests cover the two references (a scheme, a
    /// `@conn` alias) a hand-rolled key got wrong. A key that stops matching
    /// reads exactly like a document nobody registered, which is the least
    /// debuggable failure the registry can produce.
    #[test]
    fn the_registry_key_is_the_document_resolved_against_the_program() {
        let mut db = new_db();
        let file = fossil_base::SourceFile::new(
            &db,
            src_naming("shapes/person.shex"),
            "examples/nested/prog.fossil".to_string(),
        );
        let doc = fossil_base::SourceFile::new(
            &db,
            PERSON_DOCUMENT.to_string(),
            "examples/nested/shapes/person.shex".to_string(),
        );
        fossil_base::register_file(
            &mut db,
            "examples/nested/shapes/person.shex".to_string(),
            doc,
        );

        assert!(
            resolve_target_shape(&db, first_mapping(&db, file)).is_some(),
            "the key resolves against the program's directory"
        );
    }

    /// Every way a named document fails lands on `None` here: the failure has
    /// already been reported at the binding by the time a mapping asks for its
    /// target shape.
    ///
    /// # This is the measurement the deletion rests on, so it is a test
    ///
    /// It grew from four rows to nine, one per way a program can fail to bring
    /// a shape in, because the construction sites the four deleted variants had
    /// are the kind of dead code `grep` reports as live: the `Err(…)` lines
    /// still existed in the source right up to the commit that removed them,
    /// and only running a program through this function could say whether
    /// control reached them. Nine did not. **Add a row here before concluding
    /// that some tenth way would have.**
    ///
    /// `None` exactly, and not `!Some(_)`: the previous spelling was
    /// hedging against an `Err` this function can no longer produce, and the
    /// absence of that `Err` is the whole claim.
    #[test]
    fn a_document_that_cannot_answer_leaves_the_mapping_with_no_shape_clause() {
        let good = src_naming("person.shex");
        // (what is wrong with it, the program, the path registered, its text,
        //  the substring the diagnostic raised AT THE BINDING must carry).
        let cases: [(&str, String, &str, &str, &str); 9] = [
            (
                "a document nobody registered",
                src_naming("missing.shex"),
                "person.shex",
                PERSON_DOCUMENT,
                "the document `missing.shex` is not registered",
            ),
            (
                "a document no decoder claims — here by extension",
                src_naming("person.unknown"),
                "person.unknown",
                PERSON_DOCUMENT,
                "`io.shex` does not read `person.unknown`",
            ),
            (
                "a document the decoder rejected",
                src_naming("broken.shex"),
                "broken.shex",
                "!malformed expected a shape line\n",
                "expected a shape line",
            ),
            (
                "a document that decodes to no shapes at all",
                src_naming("empty.shex"),
                "empty.shex",
                "\n",
                "the binding names 1 shape(s) and the document declares 0",
            ),
            (
                "a local name nobody bound — the misspelling that used to reach \
                 the document and come back `Undeclared`",
                good.replace(": Person from", ": Persn from"),
                "person.shex",
                PERSON_DOCUMENT,
                "`Persn` is not a shape this program declares",
            ),
            (
                "a provider that does not read types",
                good.replace("io.shex(", "io.csv("),
                "person.shex",
                PERSON_DOCUMENT,
                "`io.csv` does not read types",
            ),
            (
                "a constructor this host does not install",
                good.replace("io.shex(", "io.nope("),
                "person.shex",
                PERSON_DOCUMENT,
                "`io.nope` is not a provider this host installs",
            ),
            (
                "a bare string where a provider call belongs",
                good.replace("io.shex(\"person.shex\")", "\"person.shex\""),
                "person.shex",
                PERSON_DOCUMENT,
                // Not "names no document": the binding DOES name one, and the
                // refusal says so and tells you the spelling it wanted. This
                // row's expectation was written off the reachability probe,
                // which printed the `Result` and not the diagnostic text — so
                // it guessed the message of the neighbouring case. The cause
                // is what has to be pinned, and this is the cause.
                "`person.shex` is named by no provider",
            ),
            (
                "two names and one declared shape — the second binds nothing",
                good.replace("type { Person }", "type { Person, City }")
                    .replace(": Person from", ": City from"),
                "person.shex",
                PERSON_DOCUMENT,
                "the binding names 2 shape(s) and the document declares 1",
            ),
        ];

        for (wrong, src, path, text, expected) in cases {
            let (db, file) = db_with_document(&src, path, text);
            assert!(
                resolve_target_shape(&db, first_mapping(&db, file)).is_none(),
                "{wrong}: the binding failed, so the mapping has no output \
                 contract and no error of its own — got an answer that is not \
                 `None` for:\n{src}"
            );
            let diagnostics =
                crate::lower::lower_to_hir::accumulated::<fossil_base::Diagnostic>(&db, file);
            assert!(
                diagnostics.iter().any(|d| d.message().contains(expected)),
                "{wrong}: and the failure is reported where it happened, by \
                 cause — expected {expected:?}, got: {:?}",
                diagnostics.iter().map(|d| d.message()).collect::<Vec<_>>(),
            );
        }
    }

    /// The whole reason the document became a Salsa input: editing it must
    /// change what the checker sees. Through `System::read_file` it did not —
    /// a host read registers no dependency, so the memoized answer stood.
    #[test]
    fn editing_the_document_changes_the_resolved_shape() {
        use salsa::Setter as _;

        let (mut db, file) =
            db_with_document(&src_naming("person.shex"), "person.shex", PERSON_DOCUMENT);
        let constraints = |db: &fossil_base::FossilDb| {
            resolve_target_shape(db, first_mapping(db, file))
                .expect("the program names a document")
                .constraints
                .len()
        };
        assert_eq!(constraints(&db), 1);

        let doc = fossil_base::file_at(&db, "person.shex").expect("registered");
        doc.set_text(&mut db).to(format!(
            "{PERSON_DOCUMENT}prop http://example.org/age integer 1 1\n"
        ));

        assert_eq!(
            constraints(&db),
            2,
            "the edited document is what the checker checks against"
        );
    }
}
