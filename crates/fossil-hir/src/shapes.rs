//! Target-shape resolution.
//!
//! Maps a mapping's declared shape IRI to a checker-internal [`ResolvedShape`]
//! carrying the per-predicate constraint table, converted from the
//! format-neutral [`fossil_graph_schema::Shape`].
//!
//! # The compiler is handed a value, not a parser
//!
//! This module names [`fossil_graph_schema::shapes`]'s vocabulary and no type
//! of any schema language: a host installs a [`fossil_base::Provider`] row, its
//! decode lowers its own syntax into [`fossil_graph_schema::OutputShapes`], and
//! the checker never learns which language the document was written in.
//!
//! # The types the program has
//!
//! [`resolve_target_shape`] reads the type the PROGRAM has: one it declares
//! (`crate::declared`), or one it imports with `type { … } := io.shex("…")`.
//! For an import it reads the shape document — not a descriptor threaded in by
//! the host, which an in-query caller has none of. The document is a
//! [`fossil_base::SourceFile`] **input** found through [`fossil_base::file_at`]
//! and decoded by the tracked [`crate::shape_documents::shape_document`], so an
//! edit to the `.shex` invalidates every mapping checked against it; bytes read
//! with `System::read_file` register no Salsa dependency, and in an editor that
//! is a diagnostic that never clears.
//!
//! Having no type is an ERROR: a property key is a bare name whose meaning is
//! the name of a predicate **a type declares**, so a program that neither
//! declares nor imports one cannot write a single property. `lower_to_hir`
//! reports it, at the header whose name bound nothing.
//!
//! # Every failure is reported at the BINDING, and none of them here
//!
//! A document nothing registered, one no decoder reads, one that did not
//! parse, one that does not declare the shape the mapping targets: none of them
//! is an error of this module, and the reason is the ORDER.
//!
//! A mapping header names a bare LOCAL name. [`crate::def_map`] binds those
//! names BY NAME, against this module's own [`decoded_document`], before
//! anything asks a mapping for its target shape. So a document that cannot
//! answer fails THERE: the binding takes a
//! [`ShapeBindError`](crate::def_map::ShapeBindError), `lookup_type` answers
//! `None`, [`crate::lower`] reports it by cause (`unbound_shape_problem`, which
//! carries the did-you-mean over the names the program bound) and leaves
//! `shape_iri` empty — and an empty shape IRI reads here as «no shape clause».
//! And a binding hands over a shape the document DECLARES, so the IRI can only
//! be one the document declares.
//!
//! `a_document_that_cannot_answer_leaves_the_mapping_with_no_shape_clause`
//! drives every way a program can fail to bring a shape in through
//! [`resolve_target_shape`], one row per way, plus the no-`type`-line case
//! beside it, and every one answers `None`.
//!
//! `None` therefore means two things: a mapping with NO SHAPE CLAUSE, and a
//! mapping whose shape name bound nothing and has already been told so.
//!
//! # Why the failure is returned and not emitted here
//!
//! This is a plain-Rust helper that IDE features call directly, with no sink
//! to report into and no use for one. So it returns the failure, exactly as [`crate::def_map`]'s
//! [`ShapeBindError`](crate::def_map::ShapeBindError) already does for the
//! source side, and [`crate::check`] — which runs inside `typecheck_mapping` —
//! turns it into a diagnostic.

use fossil_graph_schema::{
    Occurs, OutputShapes, Primitive, Problem, PropertyConstraint, Rejection, Shape, Span, Term,
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
    /// The name a body writes for it — [`PropertyConstraint::name`].
    pub name: SmolStr,
    /// The predicate IRI (fully-resolved string form).
    pub predicate: SmolStr,
    /// The type a value must have to satisfy this constraint, or `None` when
    /// **the document did not narrow the value type** — see
    /// [`expected_value_ty`], which is where that is decided.
    pub value_ty: Option<Ty<'db>>,
    /// The RDF term the document declares for the values —
    /// [`PropertyConstraint::term`], carried through untouched.
    pub term: Option<Term>,
    /// How many values the property may carry.
    pub occurs: Occurs,
    /// Where the DOCUMENT declares this predicate — see
    /// [`fossil_graph_schema::PropertyConstraint::span`]. Paired with
    /// [`ResolvedShape::document`], it is a label in the `.shex`.
    pub span: Option<Span>,
    /// The language ranges a tagged value must match —
    /// [`PropertyConstraint::languages`]. Empty is any.
    pub languages: Vec<String>,
}

/// One predicate of a mapping's target shape, by the short name a body writes —
/// a row of [`ResolvedShape::short_names`]' table.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Predicate {
    /// The short name: the last segment of the predicate's IRI.
    pub name: SmolStr,
    /// The predicate IRI, fully resolved.
    pub iri: SmolStr,
    /// The RDF term the document declares for the values.
    pub term: Option<Term>,
}

/// A mapping's resolved target shape — checker-internal.
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
    /// The file the declaration is in — the program for a declared type, the
    /// registered document for an imported one — so a reader that goes to a
    /// definition asks the shape where it lives instead of knowing which kind of
    /// type it is. `None` when the document is named and not registered.
    pub file: Option<fossil_base::SourceFile>,
    /// What the decoder could not lower, filtered to this shape;
    /// [`crate::check::Checker::surface_shape_lowering_errors`] surfaces these
    /// as diagnostics on the consuming mapping.
    pub rejections: Vec<Rejection>,
}

impl<'db> ResolvedShape<'db> {
    /// A diagnostic's label at a declaration of this shape: where it is, in the
    /// document that declares it. `None` for a type declared in `program`
    /// itself, where the diagnostic's own underline is already in the file the
    /// reader is looking at.
    #[must_use]
    pub fn label_at(
        &self,
        span: Option<Span>,
        program: fossil_base::SourceFile,
    ) -> Option<(Span, SmolStr)> {
        if self.file == Some(program) {
            return None;
        }
        Some((span?, self.document.clone()))
    }

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
        file: Option<fossil_base::SourceFile>,
    ) -> Self {
        let constraints = shape
            .properties
            .iter()
            .map(|c| ShapeConstraint {
                name: SmolStr::from(c.name.as_str()),
                predicate: SmolStr::from(c.predicate.as_str()),
                value_ty: expected_value_ty(db, c),
                term: c.term.clone(),
                occurs: c.occurs,
                span: c.span,
                languages: c.languages.clone(),
            })
            .collect();
        Self {
            constraints,
            span: shape.span,
            document,
            file,
            rejections,
        }
    }

    /// The shape's predicates by the SHORT NAME a program writes, and the
    /// collisions that make some of them unwritable.
    ///
    /// The short name is [`ShapeConstraint::name`] — for a document, the last
    /// segment of the IRI; for a declared type, the field's own name. It is the
    /// same name `OutputShapes::to_graph_schema` emits the column with, which is
    /// what stops a predicate being written under one name and emitted under
    /// another.
    ///
    /// **A collision is an error naming both IRIs, and never a numeric suffix.**
    /// The evidence is the authors' own: FSharp.Data's PLDI-2016 paper declares
    /// in §6.5 that its algorithm avoids anything where «a small change in the
    /// sample causes a large change in the provided types», and then does not
    /// apply that criterion to names — its collision scheme is `PascalCase2`,
    /// its singulariser produced `Purchasis` and `Sourcis`, and a minor version
    /// renamed members and broke a program in production. The repair here is
    /// the author's: declare the type in the program, where each field writes
    /// its own name beside its IRI.
    ///
    /// Both halves come back from one walk because the caller needs both: the
    /// table to resolve what the body wrote, and the collisions to say why a
    /// name it could not resolve is unwritable rather than unknown.
    #[must_use]
    pub fn short_names(&self) -> (Vec<Predicate>, Vec<NameCollision>) {
        let mut table: Vec<Predicate> = Vec::with_capacity(self.constraints.len());
        let mut collisions: Vec<NameCollision> = Vec::new();
        for c in &self.constraints {
            let short = c.name.clone();
            if let Some(first) = table.iter().find(|p| p.name == short) {
                collisions.push(NameCollision {
                    name: short,
                    first: first.iri.clone(),
                    second: c.predicate.clone(),
                });
            } else {
                table.push(Predicate {
                    name: short,
                    iri: c.predicate.clone(),
                    term: c.term.clone(),
                });
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
/// Three cases:
///
/// 1. **A narrowed literal** (`datatype: Some(p)`) expects that primitive.
/// 2. **An edge** (`targets` non-empty) expects a [`TyKind::Ref`] to the shapes
///    the constraint names: the value is a reference to another node, and
///    naming WHICH is what stops every reference being one type.
/// 3. **Anything else** — the document mentioned the predicate and said nothing
///    about its value — expects NOTHING. The checker enforces the cardinality
///    and leaves the type alone.
///
/// Case 3 is `None` — "any value", the WIDEST type, and not the narrowest: a
/// shape that declines to narrow the value must not reject a `String`, or
/// `name = Row.name` fails against `ex:name .` — a constraint that constrains
/// nothing. `PropertyConstraint::datatype`'s doc in `fossil-graph-schema` says
/// the same. A shape that does not narrow the value does not narrow it.
fn expected_value_ty<'db>(db: &'db dyn fossil_base::Db, c: &PropertyConstraint) -> Option<Ty<'db>> {
    if let Some(p) = c.datatype() {
        return Some(Ty::new(db, TyKind::Primitive(p)));
    }
    if c.targets.is_empty() {
        None
    } else {
        // The constraint's destinations ARE the type. «Some IRI» would make
        // every reference in the language one type, so an edge to the wrong
        // shape would type-check.
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
/// is a declared type, whose field names the author writes into the program as
/// constants. So this may be wrong, and being wrong costs an edit rather than a
/// silent rename.
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
/// (so editing the document re-runs the checker). Neither is true of
/// `System::read_file`.
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
pub fn decoded_document(
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
#[must_use]
pub fn inner_primitive<'db>(db: &'db dyn fossil_base::Db, ty: Ty<'db>) -> Option<Primitive> {
    match ty.kind(db) {
        TyKind::Primitive(p) => Some(*p),
        TyKind::Seq(inner) => inner_primitive(db, *inner),
        _ => None,
    }
}

/// Resolve a mapping's target shape against the type the PROGRAM has —
/// declared in it, or imported from a document it names.
///
/// `None` means the mapping has nothing to check against. Every way a type name
/// can fail to produce a shape is reported at the use, by
/// `crate::lower::unbound_shape_problem`, and leaves the mapping's shape IRI
/// empty, so nothing is left to report here.
pub fn resolve_target_shape<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<ResolvedShape<'db>> {
    // The mapping's fully-resolved target shape IRI. A mapping with no shape
    // clause yields no resolution.
    let file = mapping.file(db);
    let hir = crate::lower::lower_to_hir(db, file);
    let shape_iri = hir.mapping(db, mapping.index(db))?.shape_iri.clone();
    if shape_iri.is_empty() {
        return None;
    }

    // The type that introduced THIS shape — not the first document the file
    // happens to name, or a program with two documents would resolve its second
    // mapping against the wrong one; the fixture is
    // `docs/programs/multi-document`.
    let entry = crate::def_map::def_map(db, file).type_of_shape(db, shape_iri.as_str())?;
    resolve_type(db, file, entry)
}

/// Resolve a type the program has — a [`TypeEntry`] of its def map — to the
/// shape it names, wherever that is declared.
///
/// What [`resolve_target_shape`] does for a mapping's header, for any position
/// that names a type: an editor asks it of the name under the cursor, and a
/// mapping asks it of its target. `None` when the name bound no shape, or the
/// document it names is not registered or does not decode.
///
/// [`TypeEntry`]: crate::def_map::TypeEntry
pub fn resolve_type<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    entry: &crate::def_map::TypeEntry,
) -> Option<ResolvedShape<'db>> {
    let shape_iri = entry.shape_iri.as_ref()?;
    // For an import this is the SECOND decode of the same document, and the
    // first one has already decided everything: `shape_iri` is set only
    // because `def_map` decoded this document and took a shape out of it. So a
    // failure here would have failed there — where it is reported, by cause —
    // and a shape the lookup misses cannot exist, because the IRI came out of
    // this document's declaration list.
    let (shapes, document, home) = if entry.declared {
        (
            crate::lower::lower_to_hir(db, file).declared(db).clone(),
            SmolStr::from(file.path(db).as_str()),
            Some(file),
        )
    } else {
        let document = entry.document.clone()?;
        let shapes = decoded_document(db, file, entry.constructor.as_deref(), &document).ok()?;
        let home = fossil_base::file_at(db, &crate::documents::registry_key(db, file, &document));
        (shapes, document, home)
    };
    let shape = shapes.lookup(shape_iri.as_str())?;

    // Filter the document's rejections to this shape so the consuming mapping
    // surfaces only its own.
    let rejections: Vec<Rejection> = shapes
        .rejections()
        .iter()
        .filter(|r| rejection_targets(r, shape_iri.as_str()))
        .cloned()
        .collect();

    Some(ResolvedShape::from_shape(
        db, shape, rejections, document, home,
    ))
}

/// Every type the program has, as one [`OutputShapes`]: its declarations, then
/// each document its imports name, in written order — what a run classifies
/// edges and cardinalities with, so a run reads the types the checker read.
///
/// `Ok(None)` for a program with no type at all. A shape two sources both carry
/// keeps its first ([`OutputShapes::new`]); the checker resolves each mapping
/// against the one its type names.
///
/// # Errors
///
/// A document an import names that [`decoded_document`] refuses — which the
/// checker has already reported, so a host only meets this on a program it was
/// told does not compile.
pub fn program_shapes(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
) -> Result<Option<OutputShapes>, Problem> {
    let dm = crate::def_map::def_map(db, file);
    if dm.types(db).is_empty() {
        return Ok(None);
    }
    let mut shapes: Vec<Shape> = crate::lower::lower_to_hir(db, file)
        .declared(db)
        .shapes()
        .cloned()
        .collect();
    let mut rejections: Vec<Rejection> = Vec::new();
    let mut read: Vec<&SmolStr> = Vec::new();
    for entry in dm.types(db) {
        let Some(document) = entry.document.as_ref().filter(|d| !read.contains(d)) else {
            continue;
        };
        read.push(document);
        let decoded = decoded_document(db, file, entry.constructor.as_deref(), document)?;
        shapes.extend(decoded.shapes().cloned());
        rejections.extend(decoded.rejections().iter().cloned());
    }
    Ok(Some(OutputShapes::new(shapes, rejections)))
}

/// `true` iff a rejection belongs to the shape identified by `shape_iri`.
///
/// [`Rejection::Disjunction`] carries the offending shape IRI; the other
/// variants are conservatively associated with every shape (they describe
/// schema-wide problems the consuming mapping should still see). Narrowing
/// [`Rejection::UnresolvedRef`] by its `in_shape` is possible, and would be a
/// change with its own test.
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
            name: "p".into(),
            predicate: "https://example.org/p".into(),
            term: datatype.map(|p: Primitive| Term::Literal(Some(p.to_xsd_iri().into()))),
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            occurs: Occurs::ONE,
            // A test fixture, not a document: no text to point into.
            span: None,
            annotations: Vec::new(),
            languages: Vec::new(),
        }
    }

    /// A constraint the document did not narrow expects NOTHING — expecting
    /// `Iri`, the narrowest type there is, a shape that constrains nothing
    /// would reject a `String`.
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
    /// primitive, and an edge expects a reference to the shapes it names,
    /// because the only thing a reference to another node can be is an IRI —
    /// of a node of one of those shapes.
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
        let diagnostics = crate::lower::lower_to_hir(&db, file).diagnostics(&db);
        assert!(
            diagnostics.iter().any(|d| d.problem
                == fossil_graph_schema::Problem::UnknownShape {
                    shape: "Person".into(),
                    declared: Vec::new(),
                }
                && d.help
                    .as_deref()
                    .is_some_and(|h| h.contains("no type names at all"))),
            "and the program is still told so, got: {:?}",
            diagnostics
                .iter()
                .map(fossil_base::Diagnostic::message)
                .collect::<Vec<_>>(),
        );
    }

    /// The registry key is the document path joined onto the PROGRAM's
    /// directory, and a host has to compute the same string or every lookup
    /// misses. It is [`crate::documents::registry_key`] on both sides —
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
    /// # This is the measurement the module's error-free shape rests on
    ///
    /// Nine rows, one per way a program can fail to bring a shape in: only
    /// running a program through this function can say whether control
    /// reaches a failure here, and none does. **Add a row here before
    /// concluding that some tenth way would.**
    ///
    /// `None` exactly, and not `!Some(_)`: this function produces no `Err`,
    /// and that absence is the whole claim.
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
                "`Person` is not a shape `empty.shex` declares — it declares none",
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
                "a name the document does not declare — it declares only `Person`",
                good.replace("type { Person }", "type { Person, City }")
                    .replace(": Person from", ": City from"),
                "person.shex",
                PERSON_DOCUMENT,
                "`City` is not a shape `person.shex` declares — it declares `Person`",
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
            let diagnostics = crate::lower::lower_to_hir(&db, file).diagnostics(&db);
            assert!(
                diagnostics.iter().any(|d| d.message().contains(expected)),
                "{wrong}: and the failure is reported where it happened, by \
                 cause — expected {expected:?}, got: {:?}",
                diagnostics
                    .iter()
                    .map(fossil_base::Diagnostic::message)
                    .collect::<Vec<_>>(),
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
