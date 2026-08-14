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
//! [`fossil_base::shape_document`], so an edit to the `.shex` invalidates every
//! mapping checked against it.
//!
//! Naming no document is an ERROR now, and it used to be the one `None` this
//! module defended: it was once a DECISION — the program writes its corpus and
//! nothing is checked. The ruling of 2026-08-11 made it an error, because a
//! property key is a bare name whose meaning is the last segment of a predicate
//! IRI **the document declares**, so a program with no document cannot write a
//! single property. [`TargetShapeError::NoDocument`] is that case.
//!
//! The other four failures were `.ok()?`/`?` operators collapsing onto the same
//! silent `None`: a document nothing registered, a document no decoder reads, a
//! document that did not parse, and a document that does not declare the shape
//! the mapping targets. Each has its own [`TargetShapeError`] — which is what
//! made the commonest mistake, a misspelt shape name, produce a message at all.
//!
//! The one `Ok(None)` left is a mapping with NO SHAPE CLAUSE: there is nothing
//! to resolve, and nothing to report.
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

use std::fmt;

use fossil_graph_schema::{Occurs, OutputShapes, Primitive, PropertyConstraint, Rejection, Shape};
use smol_str::SmolStr;

use crate::def_map::MappingLoc;
use crate::ty::{ShapeId, Ty, TyKind};

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
}

/// A mapping's resolved target shape — Phase-3-internal.
#[derive(Debug, Clone)]
pub struct ResolvedShape<'db> {
    /// Interned shape id (a stable handle for `TyKind::Shape`).
    pub shape_id: ShapeId,
    /// Per-predicate constraint table.
    pub constraints: Vec<ShapeConstraint<'db>>,
    /// What the decoder could not lower, filtered to this shape;
    /// [`crate::check::Checker::surface_shape_lowering_errors`] surfaces these
    /// as diagnostics on the consuming mapping.
    pub rejections: Vec<Rejection>,
}

impl<'db> ResolvedShape<'db> {
    /// Build a [`ResolvedShape`] from a decoded [`Shape`].
    ///
    /// Plain-Rust helper. `shape_id` is supplied by the caller (a stable id is
    /// minted per-mapping; Phase 3 v0.1 uses the mapping index since each
    /// mapping targets at most one shape).
    #[must_use]
    pub fn from_shape(
        db: &'db dyn fossil_base::Db,
        shape: &Shape,
        shape_id: ShapeId,
        rejections: Vec<Rejection>,
    ) -> Self {
        let constraints = shape
            .properties
            .iter()
            .map(|c| ShapeConstraint {
                predicate: SmolStr::from(c.predicate.as_str()),
                value_ty: expected_value_ty(db, c),
                occurs: c.occurs,
            })
            .collect();
        Self {
            shape_id,
            constraints,
            rejections,
        }
    }

    /// The shape's predicates by the SHORT NAME a program writes, and the
    /// collisions that make some of them unwritable.
    ///
    /// The short name is the last segment of the predicate IRI — computed by
    /// [`fossil_graph_schema::local_name`], which is the canonical one of the
    /// several this repo grew, and the only one both `#`/`/` and `:` agree on.
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
            let short = renames
                .iter()
                .find(|(iri, _)| iri == &c.predicate)
                .map_or_else(
                    || SmolStr::from(fossil_graph_schema::local_name(c.predicate.as_str())),
                    |(_, name)| name.clone(),
                );
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

    /// The predicate IRIs this shape declares — what a `@rename` has to name one
    /// of, and the list a did-you-mean is drawn from when it names none.
    pub fn predicate_iris(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().map(|c| c.predicate.as_str())
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
/// 2. **An edge** (`targets` non-empty) expects an [`TyKind::Iri`]: the value is
///    a reference to another node, and the only thing a reference can be is an
///    IRI. This is the one place the old `Iri` default was right, and it is now
///    the only place it happens.
/// 3. **Anything else** — the document mentioned the predicate and said nothing
///    about its value — expects NOTHING. The checker enforces the cardinality
///    and leaves the type alone.
///
/// Case 3 is the fix. `check.rs` resolved a missing value type with
/// `unwrap_or_else(|| Ty::new(db, TyKind::Iri))` — the NARROWEST type in the
/// lattice — while this module's own field doc said `None` meant "any value",
/// the WIDEST. Both could not be right, and `Iri` was the wrong one: a shape
/// that declines to narrow the value was rejecting a `String`, so
/// `name = .name` failed against `ex:name .` — a constraint that constrains
/// nothing. `PropertyConstraint::datatype`'s doc in `fossil-graph-schema`
/// records the contradiction and asks whoever rewrites this to choose
/// deliberately; this is the choice. A shape that does not narrow the value
/// does not narrow the value.
fn expected_value_ty<'db>(db: &'db dyn fossil_base::Db, c: &PropertyConstraint) -> Option<Ty<'db>> {
    if let Some(p) = c.datatype {
        return Some(Ty::new(db, TyKind::Primitive(p)));
    }
    if c.targets.is_empty() {
        None
    } else {
        Some(Ty::new(db, TyKind::Iri))
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

/// Why a named shape document produced no shape.
///
/// Every variant used to be the same `None` that "the program names no
/// document" produces, so the commonest mistake — a misspelt shape name — was
/// silent. Naming NO document IS in here, at the head of the list:
/// this doc said the opposite six lines above [`TargetShapeError::NoDocument`],
/// which was already there. `Ok(None)` means one thing only — the mapping has
/// no shape clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetShapeError {
    /// The program names NO shape document at all.
    ///
    /// This used to be `Ok(None)` — a decision: the program wrote its corpus
    /// and nothing was checked. The ruling of 2026-08-11 made it an error, and
    /// deliberately: a property key is a bare name whose meaning is the last
    /// segment of a predicate IRI **the document declares**, so without a
    /// document a program cannot write a single property. What was a silent
    /// no-op is a message.
    NoDocument,
    /// The program names a document and nothing is registered at that path.
    Unregistered { document: SmolStr },
    /// The document is there and no installed decoder claims it.
    Undecodable { document: SmolStr },
    /// A decoder ran and rejected the document.
    Unparseable { document: SmolStr, cause: SmolStr },
    /// The document parsed and does not declare the shape this mapping targets.
    /// `declared` is what it DOES declare, in declaration order — the list a
    /// did-you-mean is drawn from.
    Undeclared {
        document: SmolStr,
        shape: SmolStr,
        declared: Vec<SmolStr>,
    },
}

/// Why reading a shape document produced no document.
///
/// The path→document half of [`TargetShapeError`], shared with
/// [`crate::def_map`] (which reports it as a
/// [`ShapeBindError`](crate::def_map::ShapeBindError)) and [`crate::infer`]
/// (which reports it as a diagnostic). One reader of the registry, three
/// callers with three different ways of complaining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentError {
    /// Nothing is registered at the resolved path.
    Unregistered,
    /// The decode was asked for and produced nothing — an installed row that
    /// stopped reading types between the check and the call. Not reachable from
    /// a program: every way a program can get this wrong is a [`Self::Mismatch`]
    /// with the row's own words in it.
    Undecodable,
    /// **The document is named and no provider is.** `schema = "x.shex"` — the
    /// last position where a document arrived without a row to read it, and the
    /// only one that ever selected a decoder by extension.
    Unnamed,
    /// **The row the program named refused the job**, in the row's own words:
    /// it does not read types at all (`io.csv`), or it does not accept this
    /// extension (`io.shex("catalogue.ttl")`). Ruling 13: the row checks and the
    /// row words its own rejection; the core only carries it.
    Mismatch(SmolStr),
    /// A decoder ran and rejected the document.
    Unparseable(SmolStr),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unregistered => f.write_str("no document is registered at that path"),
            Self::Undecodable => f.write_str("no installed provider reads this document"),
            Self::Unnamed => f.write_str(
                "a shape document is named by a provider call — write \
                 `schema = io.shex(\"…\")` or `schema = io.shacl(\"…\")`",
            ),
            Self::Mismatch(message) => f.write_str(message),
            Self::Unparseable(cause) => write!(f, "the decoder rejected it: {cause}"),
        }
    }
}

/// The decoded document `path` names, read as the language `constructor` names.
///
/// The ONE place `fossil-hir` turns a path a program wrote into a shape
/// document. Both steps are Salsa-visible — [`fossil_base::file_at`] reads the
/// registry input (so registering the file later invalidates a reader that
/// missed) and [`fossil_base::shape_document`] is tracked on the file's text
/// (so editing the document re-runs the checker). Neither was true of the
/// `System::read_file` this replaces.
///
/// # The row is chosen by NAME
///
/// `constructor` is what the program wrote — `io.shex`, `io.shacl` — and it
/// selects the row (ruling 13 of `SURFACE-PLAN.md`). The row is then asked two
/// questions it answers about itself, and each refusal comes back in the row's
/// own words: does it read TYPES at all, and does it accept this extension. Both
/// used to be unaskable, because the extension chose the row and the
/// constructor was discarded in `def_map`.
///
/// `constructor` is `None` only for a program that named a document without a
/// provider, and that is [`DocumentError::Unnamed`] — not a fallback. The
/// `schema =` argument of a source is a provider call like every other position
/// that names a document, so there is one rule and no corner left in it.
pub(crate) fn decoded_document(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    constructor: Option<&str>,
    path: &str,
) -> Result<OutputShapes, DocumentError> {
    use fossil_base::providers::{Capability, provider};

    let table = db.system().providers();
    let ctor = constructor.ok_or(DocumentError::Unnamed)?;
    let row = provider(table, ctor).ok_or_else(|| {
        DocumentError::Mismatch(SmolStr::from(format!(
            "`{ctor}` is not a provider — this host installs {}",
            installed(table)
        )))
    })?;
    if !row.provides(Capability::ReadTypes) {
        return Err(DocumentError::Mismatch(SmolStr::from(
            row.decline_capability(Capability::ReadTypes, table),
        )));
    }
    if !row.accepts(path) {
        return Err(DocumentError::Mismatch(SmolStr::from(
            row.decline_extension(path),
        )));
    }

    let resolved = crate::def_map::resolve_relative(db, file, path);
    let doc =
        fossil_base::file_at(db, &resolved.to_string_lossy()).ok_or(DocumentError::Unregistered)?;
    let shapes =
        fossil_base::shape_document(db, doc, row.name).ok_or(DocumentError::Undecodable)?;
    malformed_cause(&shapes).map_or(Ok(shapes), |cause| Err(DocumentError::Unparseable(cause)))
}

/// The installed constructors, for a message that names an unknown one.
fn installed(table: &[&'static fossil_base::Provider]) -> String {
    table
        .iter()
        .map(|p| format!("`{}`", p.constructor()))
        .collect::<Vec<_>>()
        .join(", ")
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

/// Map a Fossil [`Primitive`] to its `GraphAr` data-type spelling — the same
/// vocabulary [`fossil_sinks::manifest::data_type_name`] emits. A materializer
/// spelling, so it lives with the compiler and not on the lattice; the xsd
/// direction is [`Primitive::to_xsd_iri`], which does.
#[must_use]
pub const fn primitive_to_graphar(p: Primitive) -> &'static str {
    match p {
        Primitive::Integer => "int64",
        Primitive::Float => "double",
        Primitive::Bool => "bool",
        Primitive::Date => "date",
        Primitive::DateTime => "timestamp",
        Primitive::Time => "time",
        // String / AnyUri / GYear have no narrower GraphAr spelling.
        Primitive::String | Primitive::AnyUri | Primitive::GYear => "string",
    }
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
/// `Ok(None)` means the mapping has no shape clause — the one remaining case
/// where there is nothing to check against and nothing to report.
///
/// # Errors
///
/// [`TargetShapeError`], and the list grew by one at its head: the program
/// names NO document ([`TargetShapeError::NoDocument`], ruling 3 of
/// 2026-08-11), nothing is registered at the path it names, no decoder reads
/// it, it did not parse, or it does not declare the shape this mapping targets.
/// Every one of those used to be the same silent `None`.
pub fn resolve_target_shape<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Result<Option<ResolvedShape<'db>>, TargetShapeError> {
    // The mapping's fully-resolved target shape IRI (prefix already expanded by
    // `lower_to_hir`). A mapping with no shape clause yields no resolution.
    let file = mapping.file(db);
    let hir = crate::lower::lower_to_hir(db, file);
    let Some(hir_mapping) = hir.mappings(db).get(mapping.index(db)) else {
        return Ok(None);
    };
    let shape_iri = hir_mapping.shape_iri.clone();
    if shape_iri.is_empty() {
        return Ok(None);
    }

    // The document that declared THIS shape — the binding whose `type { … } :=
    // io.shex("…")` introduced it, not the first document the file happens to
    // name. A program with two documents resolved its second mapping against the
    // wrong one until `shape_binding_for` existed; the fixture is
    // `apps/docs/programs/multi-document`. A program that names none has no
    // output contract.
    let dm = crate::def_map::def_map(db, file);
    let Some((constructor, document)) = dm.shape_binding_for(db, shape_iri.as_str()) else {
        return Err(TargetShapeError::NoDocument);
    };

    let shapes =
        decoded_document(db, file, constructor.as_deref(), document.as_str()).map_err(|e| {
            match e {
                DocumentError::Unregistered => TargetShapeError::Unregistered {
                    document: document.clone(),
                },
                // A provider mismatch lands here rather than in a variant of its
                // own, and that is a deliberate limit: the message that names
                // the constructor and the extension is emitted at the BINDING,
                // where the `io.shex("…")` node gives it a real span
                // (`crate::lower::check_type_binding_provider`). Per-mapping,
                // all that is left to say is that the contract could not be
                // read.
                DocumentError::Undecodable
                | DocumentError::Unnamed
                | DocumentError::Mismatch(_) => TargetShapeError::Undecodable {
                    document: document.clone(),
                },
                DocumentError::Unparseable(cause) => TargetShapeError::Unparseable {
                    document: document.clone(),
                    cause,
                },
            }
        })?;

    let Some(shape) = shapes.lookup(shape_iri.as_str()) else {
        return Err(TargetShapeError::Undeclared {
            document,
            shape: shape_iri,
            declared: shapes
                .shapes()
                .map(|s| SmolStr::from(s.iri.as_str()))
                .collect(),
        });
    };

    // Filter the document's rejections to this shape so the consuming mapping
    // surfaces only its own.
    let rejections: Vec<Rejection> = shapes
        .rejections()
        .iter()
        .filter(|r| rejection_targets(r, shape_iri.as_str()))
        .cloned()
        .collect();

    // Phase 3 v0.1 mints a stable per-mapping shape id from the mapping index
    // (each mapping targets at most one shape).
    let shape_id = ShapeId::placeholder(u32::try_from(mapping.index(db)).unwrap_or(u32::MAX));
    Ok(Some(ResolvedShape::from_shape(
        db, shape, shape_id, rejections,
    )))
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
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    use fossil_base::test_support::{PERSON_DOCUMENT, db_with_document, new_db};

    /// Every `GraphAr` spelling is reachable, and the `String`-shaped corner of
    /// the lattice collapses on purpose. The xsd direction is not tested here —
    /// it is one function in `fossil-graph-schema`, tested there.
    #[test]
    fn graphar_spelling_covers_the_lattice() {
        assert_eq!(primitive_to_graphar(Primitive::Integer), "int64");
        assert_eq!(primitive_to_graphar(Primitive::DateTime), "timestamp");
        assert_eq!(primitive_to_graphar(Primitive::AnyUri), "string");
        assert_eq!(primitive_to_graphar(Primitive::GYear), "string");
    }

    // --- the value type a constraint expects (defect 1) --------------------

    fn prop(datatype: Option<Primitive>, targets: &[&str]) -> PropertyConstraint {
        PropertyConstraint {
            predicate: "https://example.org/p".into(),
            datatype,
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            occurs: Occurs::ONE,
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
            edge.kind(&db),
            &TyKind::Iri,
            "an edge's value is the referenced subject's IRI"
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
            .expect("the document declares the mapping's target shape")
            .expect("the program names a document, so there is something to check against");
        assert!(
            resolved.constraint_for("http://example.org/name").is_some(),
            "the resolved shape must carry the ex:name constraint"
        );
    }

    /// Naming no document was a DECISION: the program wrote its corpus and
    /// nothing was checked. The ruling of 2026-08-11 made it an error, and this
    /// test is that rule: a property key is the last segment of a predicate IRI
    /// that a shape declares, so a program with no shape document cannot write
    /// a property at all.
    ///
    /// **The message moved, and this is the test that says where to.** It used
    /// to be `TargetShapeError::NoDocument`, raised here. A header names a BARE
    /// NAME now, and a bare name is resolved by `def_map`'s type bindings
    /// before this function ever runs: a program with no `type` line binds no
    /// name, `lower_to_hir` reports it and leaves `shape_iri` empty, and an
    /// empty shape IRI reads here as «no shape clause» — `Ok(None)`. So the
    /// rule is enforced one layer up, and `NoDocument` is unreachable. See the
    /// tombstone below for the other four.
    #[test]
    fn a_program_that_names_no_document_is_an_error_now() {
        let (db, file) = db_with_document(SRC_WITHOUT_DOCUMENT, "unused.shex", PERSON_DOCUMENT);
        // `!Ok(Some(_))`, not `== Ok(None)`: what the rule of 2026-08-11 says
        // is that such a program gets no output contract and is TOLD so, and
        // both halves below hold whether this returns `Ok(None)` (what it does)
        // or is repaired to return an error again. Pinning the exact variant
        // would make this fixture decide which, and that is not a fixture's
        // call.
        assert!(
            !matches!(
                resolve_target_shape(&db, first_mapping(&db, file)),
                Ok(Some(_))
            ),
            "the name bound nothing, so there is no output contract"
        );
        let diagnostics =
            crate::lower::lower_to_hir::accumulated::<fossil_base::Diagnostic>(&db, file);
        assert!(
            diagnostics.iter().any(|d| d
                .message
                .contains("`Person` is not a shape this program declares")
                && d.message.contains("no shape names at all")),
            "and the program is still told so, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
    }

    /// The registry key is the document path joined onto the PROGRAM's
    /// directory, and a host has to compute the same string or every lookup
    /// misses. `fossil_ide::shape_documents::registry_key` is the other half of
    /// this and its module docs name the agreement; this is the half that lives
    /// where the lookup happens. A key that stops matching reads exactly like a
    /// document nobody registered, which is the least debuggable failure the
    /// registry can produce.
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
            resolve_target_shape(&db, first_mapping(&db, file))
                .expect("the key resolves against the program's directory")
                .is_some()
        );
    }

    // --- the four failures that used to be one silent `None` (defect 2) ----
    //
    // Four tests stood here, one per way a NAMED document fails to produce a
    // shape: `a_shape_the_document_does_not_declare_says_which_ones_it_does`
    // (`Undeclared`), `a_document_nothing_registered_is_its_own_failure`
    // (`Unregistered`), `a_document_no_decoder_reads_is_its_own_failure`
    // (`Undecodable`) and `a_document_the_decoder_rejected_carries_the_reason`
    // (`Unparseable`). Each asserted its own `TargetShapeError` variant.
    //
    // None of the four can fire any more, and it is not the fixtures that
    // changed — it is the ORDER. A header names a bare name, `def_map` binds
    // names POSITIONALLY against the same decoded document this function reads,
    // and every one of the four failures happens THERE first: the binding gets
    // a `ShapeBindError`, `lookup_type` answers `None`, `lower_to_hir` reports
    // it and leaves `shape_iri` empty, and this function's first guard reads an
    // empty IRI as «no shape clause» and returns `Ok(None)`. `Undeclared` is
    // doubly unreachable: positional binding takes the Nth shape the document
    // DECLARES, so the IRI it hands over is one the document declares by
    // construction, and a misspelt LOCAL name binds nothing at all.
    //
    // The four CAUSES are still covered, by the tests that own them:
    // `def_map`'s `ShapeBindError` tests, and `lower.rs`'s
    // `unbound_shape_message`, which turns each into a sentence naming the
    // document and the reason. What is left uncovered is the mapping from a
    // cause to a `TargetShapeError`, and that is because there is no longer a
    // path to it — reported as a defect, not repaired here: five of
    // `TargetShapeError`'s variants and the five arms of
    // `check::surface_target_shape_error` that render them are dead code.
    //
    // The one thing below them that IS live is the collapse itself, and this is
    // the replacement test for it.

    /// Every way a named document fails now lands on `Ok(None)` here.
    ///
    /// Written to REPLACE the four above, and it asserts what is true rather
    /// than what they wanted: the failure has already been reported at the
    /// binding by the time a mapping asks for its target shape.
    #[test]
    fn a_document_that_cannot_answer_leaves_the_mapping_with_no_shape_clause() {
        // (the program, the path registered, the text registered).
        let cases: [(String, &str, &str); 4] = [
            // A document nobody registered.
            (src_naming("missing.shex"), "person.shex", PERSON_DOCUMENT),
            // A document no decoder claims — here by extension.
            (
                src_naming("person.unknown"),
                "person.unknown",
                PERSON_DOCUMENT,
            ),
            // A document the decoder rejected.
            (
                src_naming("broken.shex"),
                "broken.shex",
                "!malformed expected a shape line\n",
            ),
            // A local name nobody bound — the misspelling that used to reach
            // the document and come back `Undeclared`.
            (
                src_naming("person.shex").replace(": Person from", ": Persn from"),
                "person.shex",
                PERSON_DOCUMENT,
            ),
        ];

        for (src, path, text) in cases {
            let (db, file) = db_with_document(&src, path, text);
            // `!Ok(Some(_))` rather than the exact variant — see
            // `a_program_that_names_no_document_is_an_error_now`. Today it is
            // `Ok(None)`; whether that is right is the open question the
            // tombstone above raises, and this assertion holds either way.
            assert!(
                !matches!(
                    resolve_target_shape(&db, first_mapping(&db, file)),
                    Ok(Some(_))
                ),
                "the binding failed, so the mapping has no output contract: {src}"
            );
            let diagnostics =
                crate::lower::lower_to_hir::accumulated::<fossil_base::Diagnostic>(&db, file);
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.message.contains("checked against nothing")),
                "and the failure is reported where it happened, got: {:?}",
                diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
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
                .expect("resolves")
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
