//! Bidirectional type checker — the real `synth`/`check`/`compatible`
//! algorithm, where a pointer-equality stub used to stand.
//!
//! The architectural keystone: [`typecheck_mapping`] is the ONLY Salsa-tracked
//! entry per mapping. `synth` / `check` / `check_property` / `lookup_field` /
//! `compatible` are plain-Rust helpers called from inside it — this preserves
//! the `MAX_PER_MAPPING_FAN_OUT = 1` invariant (LOAD-BEARING).
//!
//! [`crate::provenance::expr_types`] is a thin accessor over
//! [`typecheck_mapping`]'s output, and not the other way round:
//! `typecheck_mapping` is the source of truth for per-expression types;
//! `expr_types` is the projection.
//!
//! # Forward propagation
//!
//! When the host has registered a descriptor for the mapping's source URI, or
//! the source names a shape document,
//! [`crate::infer::resolve_source_scope`] builds the scope whose
//! [`flat`](crate::ty::Rows::flat) is a `Record` type for the source
//! row; `.field` accesses resolve against it, with did-you-mean on a miss. A
//! source that has neither has no row, and `.field` synthesises nothing — see
//! [`crate::infer`]'s module docs for the priority order.
//!
//! # Backward checking
//!
//! When a target shape is resolved (see [`crate::shapes`]), `check_property`
//! matches each property's predicate against the shape's constraint table and
//! runs [`compatible`] against the type it declares; a predicate the shape
//! REQUIRES and the body never wrote is the other direction, and
//! [`Checker::check_required_properties`] is where the declared cardinality is
//! read. The shape is a decoded document, not a schema language: nothing here
//! names one.
//!
//! # No silent coercion
//!
//! Every type mismatch emits a [`Diagnostic`] via [`delay_span_bug`] AND
//! produces an [`ErrorGuaranteed`]. `typecheck_mapping` returns `Err` when the
//! body has even one type error.

use fossil_base::{
    Diagnostic, ErrorGuaranteed, Severity, SourceFile, Span, SpanFrame, delay_span_bug,
};
use salsa::Accumulator;
use smol_str::SmolStr;

use crate::body::{ExprId, body};
use crate::def_map::{MappingLoc, def_map};
use crate::didyoumean::did_you_mean;
use crate::display::{op_text, un_op_text};
use crate::infer::resolve_source_scope;
use crate::lower::{
    BinOp, HirExpr, HirProperty, InterpolationPart, PropertyKey, UnOp, lower_to_hir,
};
use crate::provenance::{ExprTypeEntry, ExprTypes, Provenance, ProvenanceKind};
use crate::shapes::{NameCollision, ResolvedShape, TargetShapeError, resolve_target_shape};
use crate::spans::{Spans, mapping_header_span, spans};
use crate::ty::display::render_ty_kind;
use fossil_graph_schema::{Occurs, Primitive, Rejection};

use crate::ty::{Rows, Ty, TyKind};

// `BlamePos` stood here — a two-variant enum naming which side of a two-span
// blame a position referred to. Both variants are gone, for opposite reasons.
//
// `Expr(ExprId)` had no constructor outside two subtyping tests, which passed
// it as «some destination» while asserting something about the lattice. A
// destination nothing designates is not a destination.
//
// `ShapeProperty` had one constructor and no payload, so [`compatible`] fell
// back to the SOURCE expression's span and printed it into the message with
// `{:?}`: a `Span { start: 102, end: 120 }` in front of an author, naming the
// place the caret was already under. What the blame actually needs is the
// property's name — the message says which slot refused the value — and that
// is a `&str` parameter, not an enum. The second SPAN is in the `.shex`, and
// `SpanLabel` cannot yet name another file; when it can, this grows a span
// parameter rather than a variant.

/// Per-mapping type-check output. The source of truth for per-expression types
/// — `expr_types` reads it, not the reverse.
#[salsa::tracked(debug)]
pub struct TypeckOutput<'db> {
    pub expr_types: ExprTypes<'db>,
    pub source_row: Option<Ty<'db>>,
    /// The target shape's predicates by the short name a body writes —
    /// `("name", "http://xmlns.com/foaf/0.1/name")` — in declaration order.
    ///
    /// **This is how `fossil-mir` gets its IRI back.** It used to strip one out
    /// of `PropertyKey::PrefixedName`, which the CURIE put there; a bare key
    /// severs that supply and the document is the only thing that knows. MIR
    /// already reads `typecheck_mapping` for the source row, so the IRI arrives
    /// through a seam that exists, as a pair of strings — no shape vocabulary,
    /// no descriptor, and nothing of what `0e6898d` cut comes back.
    #[returns(ref)]
    pub predicates: Vec<(SmolStr, SmolStr)>,
}

/// The ONE Salsa-tracked checker entry per mapping.
///
/// Reads `body` + `spans` + the source row + the target shape,
/// runs the plain-Rust bidirectional checker, and returns a [`TypeckOutput`].
/// Returns `Err(ErrorGuaranteed)` if the body has any type error (every error
/// also pushes ≥1 [`Diagnostic`] to the accumulator).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn typecheck_mapping<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Result<TypeckOutput<'db>, ErrorGuaranteed> {
    let hir_body = body(db, mapping);
    let spans_table = spans(db, mapping);
    // A source pipeline whose row algebra does not add up taints the mapping and
    // stops here. Checking the body against a row that could not be
    // built would report a second, invented error for every property that reads a
    // column the join was supposed to bring.
    let source_scope = resolve_source_scope(db, mapping)?;
    let source_row = source_scope.as_ref().and_then(|s| s.flat(db));
    // This used to pass `ACCEPT_ALL_DEFAULT` — one literal that turned backward
    // checking off for every program compiled through the checker, because the
    // in-query path had no descriptor to thread and the argument demanded one.
    // `resolve_target_shape` now reads the document the PROGRAM names, as a
    // Salsa input, so editing that document re-runs this query.
    //
    // The per-mapping fan-out is unchanged: the two queries it adds
    // (`file_at`'s registry read and `shape_document`) are keyed by the
    // DOCUMENT, not by the mapping, so ten mappings checking against one
    // document share one decode. `tests/invalidation_regression.rs` carries the
    // full accounting.
    let resolved_shape = match resolve_target_shape(db, mapping) {
        Ok(shape) => shape,
        Err(e) => {
            surface_target_shape_error(db, mapping, &e);
            None
        }
    };

    // The short-name table, and the collisions that make some names unwritable.
    // Built once per mapping: the body resolves every key against it, and a
    // collision is reported here rather than once per property that trips on it.
    //
    // The rename table comes off the `type` binding that introduced this
    // mapping's shape, keyed by the shape IRI because that is what survives the
    // header's resolution. It is read through `def_map`, which is file-keyed and
    // structurally stable across body edits, so it adds no per-mapping fan-out.
    let renames = shape_iri_of(db, mapping)
        .map(|iri| crate::def_map::def_map(db, mapping.file(db)).renames_for_shape(db, &iri))
        .unwrap_or_default();
    let (predicates, collisions) = resolved_shape
        .as_ref()
        .map_or_else(|| (Vec::new(), Vec::new()), |s| s.short_names(&renames));
    let mut cx = Checker {
        expr: Expr {
            db,
            file: mapping.file(db),
            flat: source_row,
            rows: source_scope,
            // A body addresses its columns by the BINDING that introduced them,
            // and `from` names the relation; the two coincide only when the
            // relation is a binding that reads a file. This is provenance, so
            // it is the `from` name.
            relation: source_binding_name(db, mapping),
            spans: SpanSource::Table(spans_table),
            entries: Vec::new(),
            first_error: None,
            expected_ref: None,
        },
        mapping,
        resolved_shape,
        predicates: predicates.clone(),
        renames,
    };

    // Surface what the decoder rejected before checking the body — they are
    // informational + suggestive (they do not error the mapping out).
    cx.surface_shape_lowering_errors();
    cx.surface_name_collisions(&collisions);

    // Check each property's RHS.
    let properties = hir_body.properties(db);
    for (i, prop) in properties.iter().enumerate() {
        let expr_id = ExprId(u32::try_from(i).unwrap_or(u32::MAX));
        cx.check_property(expr_id, prop);
    }
    cx.check_required_properties(properties);

    let first_error = cx.expr.first_error;
    let expr_types = ExprTypes::new(db, cx.expr.entries);
    first_error.map_or_else(
        || Ok(TypeckOutput::new(db, expr_types, source_row, predicates)),
        Err,
    )
}

/// Report a target shape the program named and the document could not supply.
///
/// # Four arms stood here and none of them could fire
///
/// One per document failure — `Unregistered`, `Undecodable`, `Unparseable` and
/// `Undeclared` — written because each had been a silent `None`. A mapping
/// header names a bare LOCAL name now, `def_map` binds those positionally
/// against the same decoded document, and every one of the four fails THERE
/// first, so `resolve_target_shape` never returns them. They are deleted with
/// the variants; `crate::shapes::TargetShapeError` records what is left and
/// what it would take to remove it.
///
/// **The did-you-mean the `Undeclared` arm carried is not lost** — it moved to
/// `crate::lower::unbound_shape_message`, which is where a misspelt shape name
/// is reported now. Its candidates changed with it, and correctly: that arm
/// suggested over the shape IRIs a DOCUMENT declares, and what a header can
/// misspell is a local NAME the program bound.
///
/// Informational-with-teeth, like [`crate::infer`]'s treatment of a source
/// binding that resolved no shape: a `Diagnostic` is accumulated (so the CLI
/// and the LSP show it) but the mapping is not poisoned — its body is still
/// worth checking forward, and refusing to check it would report a second,
/// invented error for every property.
fn surface_target_shape_error<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    e: &TargetShapeError,
) {
    // The mapping's header — `User : ex:Persn from users` — is where every one
    // of these belongs: the shape name that did not resolve is written there,
    // and so is the absence of a document. It used to be `Span { start: 0, end:
    // 0 }`, which does not mean "no underline": the default frame is
    // `MappingRelative`, so `rebase_to_file` turns it into `base..base` and the
    // squiggle lands on the mapping's first byte — a plausible place and the
    // wrong one. The commonest case is a misspelt shape name (`ex:Persn`).
    let span = mapping_header_span(db, mapping);
    let message = match e {
        TargetShapeError::NoDocument => "this program names no shape document, so it cannot \
             write a property: a property key is the last segment of a predicate IRI that a \
             shape declares. Bring one in with `type { … } := io.shex(\"shop.shex\")`."
            .to_string(),
    };
    let _eg = delay_span_bug(db, span, message);
}

/// Render the "split into N mappings" suggestion for a value disjunction:
/// one mapping per branch, named `{base}{n}`, with one property line per
/// predicate the branch constrains.
///
/// This lived in the `ShEx` decoder and walked the `OneOf` AST node, which a
/// `SuggestionSeed` cloned and carried through the whole compiler so the
/// emitter could walk it again. [`Rejection::Disjunction`] carries the branch
/// predicates instead — the only thing the rendering ever read out of that
/// node — so the suggestion is written where it is emitted, over strings.
///
/// # Why the renderer is HERE and not in the decoder
///
/// Because three of its four arguments do not exist in a shape document. The
/// bound shape NAME, the `from` binding and the `@subject` are all read off the
/// consuming mapping's CST, and a decoder has never seen the program — it has
/// seen `ShEx`. Only the branch predicates come from the document, and those
/// cross as plain strings in [`Rejection::Disjunction`], which is why
/// `fossil-hir` needs no dependency on `fossil-shex` at all.
///
/// That was worth writing down because the opposite reasoning had been, and it
/// produced a second renderer in `fossil-shex` that outlived its own argument:
/// «re-emitting syntax stays with the decoder, because it is the decoder that
/// knows the syntax». It knew the `ShEx` syntax. It did not know Fossil's, and
/// it emitted a CURIE header, `iri =`, a backtick template and a leading `.` —
/// a quick-fix the parser refuses — while nothing in the compiler called it.
/// Deleted; this is the one implementation.
///
/// A predicate renders as the BARE NAME a body writes — the last segment of
/// its IRI. It used to render as `<absolute-iri>`, which was the
/// right call while an absolute IRI was a property key; it is not one now, so
/// the suggestion would have emitted Fossil that does not parse. The suggestion
/// is generated code and it has to compile, which
/// `the_generated_split_suggestion_compiles` is there to prove.
/// The parameters are in EMISSION order — name, shape, source, subject — and
/// that is not cosmetic: the function this replaces took the IRI template
/// second and the shape fourth, so a call site could swap the shape and the
/// `from` clause and still compile, still parse, and still pass a test that
/// only counted mappings. One did, in this commit, before this reorder.
///
/// # Every argument is SOURCE TEXT, and three of them were not
///
/// The header names a bound shape NAME — the word a `type { … } := io.shex(…)`
/// binding introduced — and never an IRI; the subject is a real `@subject`
/// right-hand side; a property's value is a QUALIFIED reference. All three were
/// written in the retired surface here, and each one alone made the emitted
/// mapping unparseable or silently short a property. `render_split_suggestion`
/// cannot check any of them — it is a formatter over strings — so what enforces
/// it is that its one production call site,
/// `Checker::surface_shape_lowering_errors`, reads all four out of the mapping
/// it is splitting rather than inventing them.
#[must_use]
pub fn render_split_suggestion(
    base_mapping_name: &str,
    base_shape_name: &str,
    base_from_clause: &str,
    base_iri_template: &str,
    disjuncts: &[Vec<String>],
    renames: &[(SmolStr, SmolStr)],
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for (i, branch) in disjuncts.iter().enumerate() {
        let idx = i + 1;
        // `write!` into a `String` is infallible.
        let _ = write!(
            out,
            "{base_mapping_name}{idx} : {base_shape_name} from {base_from_clause}\n    @subject = {base_iri_template}\n",
        );
        if branch.is_empty() {
            // A branch whose predicates the decoder could not name — a nested
            // disjunction, or a reference it did not resolve. The mapping is
            // still the right shape; the user has to fill the body in.
            out.push_str("    # TODO: this branch names no predicate — split it by hand\n");
        }
        for predicate in branch {
            // The name a body may actually write — [`fossil_graph_schema::short_name`],
            // the rename included. It was `local_name`, so a split suggested
            // for a shape whose colliding predicate the program had already
            // repaired emitted the name the repair renamed AWAY from: the
            // compiler's two generated repairs disagreeing with each other.
            let short = fossil_graph_schema::short_name(predicate, renames);
            // `{base_from_clause}.{short}`, and the qualifier is the whole
            // repair. It was `.{short}` — the retired `FieldRef`, a leading dot
            // naming a column of an anonymous current row. The parser refuses
            // it, so the property never lowered and the mapping this function
            // emitted came back with ONE property where it had written two: the
            // compiler emitting source it cannot read back. A reference is
            // qualified now, and the row it qualifies against is the binding the
            // `from` clause names.
            let _ = writeln!(out, "    {short} = {base_from_clause}.{short}");
        }
        out.push('\n');
    }
    out
}

/// Plain-Rust bidirectional checker state. NOT `#[salsa::tracked]` — lives only
/// for the duration of one [`typecheck_mapping`] call.
// `missing_debug_implementations`: `Checker` holds `&dyn fossil_base::Db`
// (the Salsa database trait object is not `Debug`) plus interned handles; a
// derived `Debug` is impossible and a hand-rolled one would add noise without
// value (the struct is a short-lived checker scratchpad, never logged).
#[allow(missing_debug_implementations)]
pub struct Checker<'db> {
    /// Everything an expression needs. A body is an expression checker plus a
    /// target shape — see [`Expr`].
    pub(crate) expr: Expr<'db>,
    pub(crate) mapping: MappingLoc<'db>,
    pub(crate) resolved_shape: Option<ResolvedShape<'db>>,
    /// The target shape's predicates by short name — what a bare property key
    /// resolves against.
    pub(crate) predicates: Vec<(SmolStr, SmolStr)>,
    /// The `@rename`s written above the binding that introduced this mapping's
    /// shape, as `(predicate IRI, the name to write instead)`. `predicates`
    /// above is this already applied; the table itself is kept because
    /// [`Checker::check_required_properties`] walks the shape's constraints and
    /// not that table, and walking them with [`fossil_graph_schema::local_name`]
    /// is how a renamed predicate the body HAD written was reported missing.
    pub(crate) renames: Vec<(SmolStr, SmolStr)>,
}

impl Checker<'_> {
    fn header_span(&self) -> Span {
        mapping_header_span(self.expr.db, self.mapping)
    }

    /// The IRI of the shape this mapping targets — what `@subject` mints a
    /// reference to.
    fn mapping_shape_iri(&self) -> Option<SmolStr> {
        shape_iri_of(self.expr.db, self.mapping)
    }

    /// Check one property: synth its RHS and, if the shape declares the
    /// predicate its key names, check against that constraint.
    ///
    /// The key is a bare name, so there is a resolution step in front of the
    /// check: `name` means the predicate of this shape whose IRI ends in
    /// `name`, and a name no predicate ends in is an error with a did-you-mean
    /// over the ones that do. That is not a lookup failure to swallow — under a
    /// mandatory shape document (ruling 3 of 2026-08-11) a key the shape does
    /// not declare is a property that would be written into a corpus nothing
    /// describes.
    pub fn check_property(&mut self, expr_id: ExprId, prop: &HirProperty) {
        // The identity is not a shape predicate: a `.shex` describes the
        // predicates of a node, and in RDF the subject IS the node. A key the
        // shape does not declare, and a mapping with no resolved shape at all,
        // both yield no expectation — each has already said so, or says so in
        // `resolve_predicate` below.
        //
        // This resolution happens BEFORE the synth, and that ordering is the
        // whole of the bidirectionality: an interpolation's type depends on
        // what is expected of it, so the expectation cannot be looked up after
        // the fact. It used to be, and the consequence was measurable — an
        // interpolated IRI in value position was `String`, unsatisfiable
        // against every predicate whose range is a shape.
        // `Subject` and the unguarded `Name(_)` both answer `None` and cannot be
        // merged: the guarded `Name(name) if self.resolved_shape.is_some()` arm
        // sits between them, and match arms are tried in order. One combined arm
        // would have to go first, and it would shadow the guard.
        #[allow(clippy::match_same_arms)]
        let expectation = match &prop.key {
            PropertyKey::Subject => None,
            PropertyKey::Name(name) if self.resolved_shape.is_some() => {
                self.resolve_predicate(expr_id, name).and_then(|pred| {
                    let shape = self.resolved_shape.as_ref()?;
                    let constraint = shape.constraint_for(pred.as_str())?;
                    // `value_ty` is passed through as an `Option`, and that IS
                    // the fix: it used to be
                    // `unwrap_or_else(|| Ty::new(db, TyKind::Iri))`, so a
                    // predicate the document declined to narrow demanded the
                    // narrowest type in the lattice and rejected every string
                    // in the corpus.
                    // The blamed name is the one the BODY wrote, not the
                    // predicate IRI `resolve_predicate` returned: the message
                    // reads back the line the author is looking at, and a
                    // rename means those two are different words.
                    Some((
                        constraint.value_ty,
                        name.clone(),
                        constraint.span.map(|span| (span, shape.document.clone())),
                    ))
                })
            }
            PropertyKey::Name(_) => None,
        };

        // Always synth the RHS so its type is recorded in `entries` (provenance
        // / hover consume this even when there is no backward constraint).
        let db = self.expr.db;
        // `@subject` mints the identity of the node THIS mapping produces, so
        // what it is expected to be is a reference to this mapping's own shape.
        // Everywhere else the expectation comes off the resolved predicate.
        self.expr.expected_ref = if matches!(prop.key, PropertyKey::Subject) {
            // An identity is ALWAYS a reference — to the shape this mapping
            // targets, or, when the program named a document that bound
            // nothing, to the empty set. That is the same answer `synth_edge`
            // gives an unresolvable target: something already said why, and a
            // reference to no shape satisfies every slot rather than blaming
            // the identity a second time.
            Some(self.mapping_shape_iri().into_iter().collect())
        } else {
            expectation.as_ref().and_then(|(ty, _, _)| {
                ty.and_then(|t| match t.kind(db) {
                    TyKind::Ref(names) => Some(names.clone()),
                    _ => None,
                })
            })
        };
        let actual = self.expr.synth(expr_id, &prop.value);
        self.expr.expected_ref = None;

        if let (Some(actual), Some((expected, property, declared_at))) = (actual, expectation) {
            // `constraint.occurs` is deliberately NOT read here. The count a
            // shape declares is checked in `check_required_properties`, over the
            // predicates the body never wrote — the only direction that can be
            // observed, now that no value type can carry «zero or one» in
            // itself.
            let type_name = self.target_type_name();
            let _ = compatible(
                &mut self.expr,
                actual,
                expected,
                expr_id,
                &prop.value,
                &property,
                declared_at.map(|(span, document)| Declared {
                    span,
                    document,
                    shape: type_name,
                }),
            );
        }
    }

    /// The predicate IRI a bare key names, or a diagnostic saying it names none.
    fn resolve_predicate(&mut self, expr_id: ExprId, name: &SmolStr) -> Option<SmolStr> {
        if let Some((_, iri)) = self.predicates.iter().find(|(n, _)| n == name) {
            return Some(iri.clone());
        }
        let db = self.expr.db;
        let candidates: Vec<&str> = self.predicates.iter().map(|(n, _)| n.as_str()).collect();
        let suggestion = did_you_mean(name.as_str(), candidates.iter().copied());
        let msg = suggestion.map_or_else(
            || {
                if candidates.is_empty() {
                    format!("the target shape declares no predicate, so there is no `{name}`")
                } else {
                    format!(
                        "the target shape declares no `{name}` — it declares {}",
                        candidates
                            .iter()
                            .map(|c| format!("`{c}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            },
            |s| format!("the target shape declares no `{name}` — did you mean `{s}`?"),
        );
        let eg = delay_span_bug(db, self.expr.span_of(expr_id), msg);
        self.expr.record_error(eg);
        None
    }

    /// Two predicates of the shape with the same short name make BOTH of them
    /// unwritable, and that is reported once per mapping rather than once per
    /// property that trips on it.
    fn surface_name_collisions(&mut self, collisions: &[NameCollision]) {
        let db = self.expr.db;
        let header_span = self.header_span();
        // **The recommendation is code, and both placeholders had to go.**
        //
        // This line said `@rename(<Type>, "…" as <another_name>)` and neither
        // half was writable: `<Type>` and `<another_name>` are not identifiers,
        // and `@rename` had no production at all — `AT_ATTR` at top level fell
        // through to `bump_as_error`. So the one repair the compiler names for
        // the one error it refuses to fix itself could not be pasted, in three
        // independent ways.
        //
        // Now the type is the bound name this mapping targets, the alias comes
        // from the vocabulary that brought the predicate in (see
        // `shapes::suggested_alias` for why a SUGGESTION is not the banned
        // derivation), and the production exists.
        // `the_recommended_rename_parses_and_repairs_the_collision` feeds this
        // exact text back through the compiler and checks that the collision
        // goes away and the renamed key resolves.
        let type_name = self.target_type_name();
        // The BINDING, not the mapping header: the collision is a fact about
        // the shape this program brought in, the reader chose it on that line,
        // and the `@rename` the help proposes goes above it. The header span is
        // the fallback for a mapping whose type name bound nothing, which has
        // already been told so.
        let bound_at = def_map(db, self.expr.file).lookup_type_span(db, type_name.as_str());
        let document = self.resolved_shape.as_ref().map(|s| s.document.clone());
        let declared_at = |iri: &SmolStr| {
            let shape = self.resolved_shape.as_ref()?;
            Some((
                shape.constraint_for(iri.as_str())?.span?,
                shape.document.clone(),
            ))
        };
        for c in collisions {
            let NameCollision {
                name,
                first,
                second,
            } = c;
            let alias = crate::shapes::suggested_alias(second);
            let mut d = Diagnostic::new(
                Severity::Error,
                format!("two predicates of {type_name} are both called `{name}`"),
                bound_at.unwrap_or(header_span),
            );
            if bound_at.is_some() {
                d = d.file_absolute();
            }
            d = d.with_label(
                bound_at.unwrap_or(header_span),
                format!("{type_name} is bound here"),
                if bound_at.is_some() {
                    SpanFrame::FileAbsolute
                } else {
                    SpanFrame::MappingRelative
                },
            );
            // **The two IRIs move out of the message and onto the lines that
            // declare them.** They were the only way to tell the reader which
            // two predicates collided, and a full IRI in the middle of a
            // sentence is the thing a caret exists to replace.
            for iri in [first, second] {
                if let Some((span, doc)) = declared_at(iri) {
                    d = d.with_document_label(span, iri.to_string(), doc);
                }
            }
            let _ = &document;
            d = d.with_help(format!(
                "a short name is the last segment of the predicate IRI, and fossil never picks \
                 between two. Give one of them another name above the binding: \
                 @rename({type_name}, \"{second}\" as {alias})"
            ));
            let eg = fossil_base::raise(db, d);
            self.expr.record_error(eg);
        }
    }

    /// Every predicate the shape requires and the body never wrote.
    ///
    /// The other direction of the backward check, and the one that was missing:
    /// `check_property` walks what the body HAS, so a body that simply omits a
    /// required predicate passed every check there is. A shape that says
    /// `shop:email xsd:string ;` — cardinality exactly one — is a promise the
    /// corpus makes to its readers, and a mapping that does not keep it writes
    /// a node that does not conform to the shape it declares.
    ///
    /// **The name compared is [`fossil_graph_schema::short_name`], not
    /// [`local_name`].** This walked the constraints with `local_name` and so
    /// did not know about `@rename`: a required predicate the program had
    /// renamed and then WRITTEN under its new name was reported missing, and
    /// the only repair the compiler offers for a name collision made the
    /// program it repaired fail to compile.
    fn check_required_properties(&mut self, properties: &[HirProperty]) {
        use std::fmt::Write as _;

        let db = self.expr.db;
        let header_span = self.header_span();
        let Some(shape) = self.resolved_shape.as_ref() else {
            return;
        };
        let missing: Vec<(SmolStr, SmolStr)> = shape
            .constraints
            .iter()
            .filter(|c| c.occurs.demands_one_or_more())
            .map(|c| {
                (
                    SmolStr::from(fossil_graph_schema::short_name(
                        c.predicate.as_str(),
                        &self.renames,
                    )),
                    c.predicate.clone(),
                )
            })
            .filter(|(short, _)| {
                !properties
                    .iter()
                    .any(|p| matches!(&p.key, PropertyKey::Name(n) if n == short))
            })
            .collect();
        // The predicates the shape declares and does NOT require — what the
        // author may legitimately leave out, which is the other half of «add
        // this one». Without it the reader has to open the document to find out
        // whether the rest of their omissions are also about to be reported.
        let optional: Vec<SmolStr> = shape
            .constraints
            .iter()
            .filter(|c| !c.occurs.demands_one_or_more())
            .map(|c| {
                SmolStr::from(fossil_graph_schema::short_name(
                    c.predicate.as_str(),
                    &self.renames,
                ))
            })
            .collect();
        let declared_at: Vec<(SmolStr, Option<(Span, SmolStr)>)> = missing
            .iter()
            .map(|(_, iri)| {
                (
                    iri.clone(),
                    shape
                        .constraint_for(iri.as_str())
                        .and_then(|c| c.span)
                        .map(|span| (span, shape.document.clone())),
                )
            })
            .collect();
        let mapping_name = self.mapping_name();
        let type_name = self.target_type_name();
        for ((short, iri), (_, at)) in missing.iter().zip(declared_at) {
            let mut d = Diagnostic::new(
                Severity::Error,
                format!("`{mapping_name}` never writes `{short}`, and {type_name} requires it"),
                header_span,
            )
            .with_label(
                header_span,
                format!("this mapping produces {type_name}"),
                SpanFrame::MappingRelative,
            );
            // The IRI is not in the message any more: it was there because
            // there was nowhere else to put it, and where it belongs is under
            // the line of the document that requires it.
            if let Some((span, doc)) = at {
                d = d.with_document_label(
                    span,
                    format!("required here: {}", render_occurs(shape_occurs(shape, iri))),
                    doc,
                );
            }
            let mut help = format!("add `{short} = ` to the body.");
            if !optional.is_empty() {
                let names: Vec<String> = optional.iter().map(|o| format!("`{o}`")).collect();
                let (list, verb) = (
                    names.join(", "),
                    if names.len() == 1 { "is" } else { "are" },
                );
                let _ = write!(help, " {list} {verb} optional and may stay out.");
            }
            let eg = fossil_base::raise(db, d.with_help(help));
            self.expr.record_error(eg);
        }
    }

    fn surface_shape_lowering_errors(&mut self) {
        let Some(shape) = self.resolved_shape.as_ref() else {
            return;
        };
        let db = self.expr.db;
        let header_span = self.header_span();
        // Clone the data we need so we don't hold a borrow of `self` across the
        // mutable `record_error` calls.
        let rejections = shape.rejections.clone();
        let renames = self.renames.clone();
        let base_name = self.mapping_name();
        let source_name = source_binding_name(self.expr.db, self.mapping);
        // The header names a bound shape NAME, not an IRI. This argument was
        // the `Rejection`'s `shape_iri`, so the suggestion emitted
        // `Contact1 : http://example.org/Contact from users` — a header the
        // parser refuses outright.
        let shape_name = self.target_type_name();
        // …and the identity is the one this mapping already declares. It was
        // the placeholder `` `${ex:}item/${.id}` ``: a backtick template with
        // `${…}` holes and a leading-dot reference, three retired spellings in
        // one argument, none of which parse. A split is a rewrite of ONE
        // mapping into N, so every branch keeps that mapping's identity — the
        // placeholder was never the right answer either, only a less visible
        // wrong one.
        let subject = self.subject_source_text();
        for rejection in &rejections {
            match rejection {
                Rejection::Disjunction {
                    shape_iri,
                    disjuncts,
                } => {
                    let suggestion = render_split_suggestion(
                        base_name.as_str(),
                        shape_name.as_str(),
                        // The `from` clause is the SOURCE BINDING, not the
                        // mapping. This argument was `base_name` — the mapping's
                        // own name — so the suggestion emitted
                        // `Contact1 : ex:Contact from Contact`, a `from` that
                        // points at the mapping being split. The docblock on
                        // `render_split_suggestion` warns about exactly this
                        // swap because it had already happened once; the
                        // parameter reorder it describes made the two arguments
                        // adjacent and did not stop them being the same string.
                        // No test saw it: the corpus passed `"users"` by hand.
                        source_name.as_str(),
                        subject.as_str(),
                        disjuncts,
                        &renames,
                    );
                    let n = disjuncts.len();
                    let msg = format!(
                        "a value disjunction is not supported in v0.1 ({n} \
                         branches in shape `{shape_iri}`); help: split into {n} \
                         separate mappings (one per branch). The \
                         split-into-mappings suggestion is provided \
                         programmatically (see `Diagnostic.suggestion_source`)."
                    );
                    // Structured suggestion carrier (Blocker #3) — NOT a
                    // Markdown delimiter. Accumulate directly (informational;
                    // no ErrorGuaranteed).
                    Diagnostic::new(Severity::Error, msg, header_span)
                        .with_suggestion_source(suggestion)
                        .accumulate(db);
                }
                Rejection::CyclicRef { path } => {
                    let eg = delay_span_bug(
                        db,
                        header_span,
                        format!("cyclic shape graph not supported: {}", path.join(" -> ")),
                    );
                    self.expr.record_error(eg);
                }
                Rejection::UnresolvedRef { label, in_shape } => {
                    let eg = delay_span_bug(
                        db,
                        header_span,
                        format!("unresolved shape ref `{label}` in shape `{in_shape}`"),
                    );
                    self.expr.record_error(eg);
                }
                Rejection::Malformed(m) => {
                    let eg =
                        delay_span_bug(db, header_span, format!("malformed shape document: {m}"));
                    self.expr.record_error(eg);
                }
            }
        }
    }

    /// The mapping's name text (for diagnostics + suggestion generation).
    fn mapping_name(&self) -> SmolStr {
        let file = self.mapping.file(self.expr.db);
        lower_to_hir(self.expr.db, file)
            .mapping(self.expr.db, self.mapping.index(self.expr.db))
            .map_or_else(|| SmolStr::from("Mapping"), |m| m.name.clone())
    }

    /// This mapping's `@subject` right-hand side, VERBATIM — the source text a
    /// generated rewrite of this mapping has to carry through.
    ///
    /// Read off the CST and not off [`crate::body::HirBody`], because the HIR
    /// is not printable: `HirExpr::Interpolation` holds lowered parts and
    /// rendering them back would be a second, unproven spelling of the surface.
    /// The text is the surface.
    ///
    /// It reads [`mapping_cst_node`] — the per-mapping invalidation barrier this
    /// file already goes through for [`mapping_header_span`] — and NEVER
    /// `parse(db, file)`, which is the whole-file read the barrier exists to
    /// keep out of the compile path.
    ///
    /// A mapping with no `@subject` cannot reach here: the identity is required,
    /// exactly one, and first (`crate::body::check_identity`). The fallback is a
    /// constant IRI, which is a legal identity — a suggestion is worth nothing
    /// if it cannot be pasted, and a missing right-hand side would emit
    /// `@subject = ` and take the parser down with it.
    fn subject_source_text(&self) -> String {
        use fossil_syntax::SyntaxKind;

        crate::body::mapping_cst_node(self.expr.db, self.mapping)
            .syntax()
            .and_then(|node| {
                node.children()
                    .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)?
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::PROPERTY)
                    .find(|p| {
                        p.descendants_with_tokens()
                            .filter_map(fossil_syntax::SyntaxElement::into_token)
                            .any(|t| t.kind() == SyntaxKind::AT_ATTR && t.text() == "@subject")
                    })?
                    .children()
                    .find(|c| c.kind() == SyntaxKind::EXPR)
                    .map(|e| e.text().to_string().trim().to_string())
            })
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "\"https://example.org/item\"".to_string())
    }

    /// The LOCAL name of the type this mapping targets — `Person`, the word a
    /// `type { Person } := …` binding introduced and the header repeats.
    ///
    /// Not the shape IRI: it is what a `@rename`'s first argument has to be, and
    /// a suggestion that quoted the IRI there would not compile.
    fn target_type_name(&self) -> SmolStr {
        let db = self.expr.db;
        let file = self.mapping.file(db);
        let Some(iri) = shape_iri_of(db, self.mapping) else {
            return SmolStr::new_static("Type");
        };
        crate::def_map::def_map(db, file)
            .types(db)
            .iter()
            .find(|t| t.shape_iri.as_deref() == Some(iri.as_str()))
            .map_or_else(|| SmolStr::new_static("Type"), |t| t.name.clone())
    }
}

/// The name of the relation a mapping draws `from` — for provenance.
pub(crate) fn source_binding_name<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> SmolStr {
    lower_to_hir(db, mapping.file(db))
        .mapping(db, mapping.index(db))
        .map_or_else(SmolStr::default, |m| m.source_binding.clone())
}

/// The fully-resolved shape IRI a mapping targets, or `None` when its header
/// did not lower.
///
/// A free function rather than a `Checker` method because `typecheck_mapping`
/// needs it BEFORE the `Checker` exists — the rename table is an input to the
/// short-name table, which is an input to the `Checker`.
fn shape_iri_of<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> Option<SmolStr> {
    let file = mapping.file(db);
    lower_to_hir(db, file)
        .mapping(db, mapping.index(db))
        .map(|m| m.shape_iri.clone())
        .filter(|iri| !iri.is_empty())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "check_tests.rs"]
mod tests;
/// Compatibility check: is `actual` a subtype of `expected`, AND does
/// `actual`'s cardinality satisfy the constraint?
///
/// A pointer-equality stub stood here first; what replaced it is `subtypes`
/// (S-Refl, S-IntFlt, S-TmplIri, S-SeqCov) + two-span blame with real spans
/// from [`Spans`]. S-Opt and S-OptCov went with `TyKind::Optional`, and the
/// cardinality check went with them — see the body.
///
/// `expected` is an `Option` because a shape document may decline to narrow a
/// predicate's value type at all (`ex:name .`), and `None` is that: the
/// cardinality still binds, the type does not. It used to arrive as
/// `TyKind::Iri` — the narrowest type in the lattice standing in for the
/// widest — so a constraint that constrained nothing rejected a `String`. See
/// [`crate::shapes::ShapeConstraint::value_ty`].
///
/// Plain-Rust (NOT `#[salsa::tracked]`). Called from within the tracked
/// [`typecheck_mapping`] frame (and from tests' tracked shims) so its
/// diagnostic emissions are valid.
///
/// `property` is the name the BODY wrote for the slot that refused the value,
/// and `source_value` the lowered right-hand side — the message names the slot,
/// the label under the caret reads the expression back and says what it is.
/// Neither used to be here: the message was `expected `Float`, got `String`
/// (expected because of the constraint at Span { start: 102, end: 120 })`, a
/// debug-printed span standing in for a second location, and that span was the
/// SOURCE's, so it pointed at the caret the reader was already looking at. A
/// location belongs in a label; a name is what a message has to carry, because
/// a body writes several properties and the caret alone does not say which
/// constraint spoke.
///
/// `declared_at` is the OTHER file — the line of the shape document that
/// declares the constraint being violated, which makes this the two-span blame
/// its name has always claimed. `None` when the document did not say where
/// (`ShExJ`, SHACL, a hand-built table), and then the report is what it was.
pub fn compatible<'db>(
    cx: &mut Expr<'db>,
    actual: Ty<'db>,
    expected: Option<Ty<'db>>,
    source_expr: ExprId,
    source_value: &HirExpr,
    property: &str,
    declared_at: Option<Declared>,
) -> Result<(), ErrorGuaranteed> {
    let db = cx.db();

    // Subtype check (`subtypes`, below). A constraint that narrows nothing is
    // satisfied by anything.
    //
    // There used to be a second check here, and an `occurs: Occurs` parameter
    // to feed it: an `Optional<X>` value could not satisfy a constraint
    // demanding 1+. `TyKind::Optional` was never CONSTRUCTED anywhere but a
    // test — `expected_value_ty` emits `Primitive` or `Iri`, and
    // `record_from_inferred` types every descriptor column bare — so the check
    // could not fire, and it is gone with the variant. The cardinality a shape
    // declares is enforced in exactly one place now, and it is the direction
    // that can be observed: [`Checker::check_required_properties`], over the
    // predicates the body never wrote.
    if expected.is_none_or(|e| subtypes(db, actual, e)) {
        return Ok(());
    }

    // Mismatch → blame the slot by name, underline the value.
    let source_span = cx.span_of(source_expr);

    let actual_display = render_ty_kind(db, actual.kind(db));
    // Only reachable with `Some(_)`: a constraint that narrows nothing cannot
    // fail the subtype check.
    let expected_display = expected.map_or_else(
        || "any value".to_string(),
        |e| render_ty_kind(db, e.kind(db)),
    );

    let frame = cx.frame();
    let mut d = Diagnostic::new(
        Severity::Error,
        format!("`{property}` expects {expected_display}, and this is {actual_display}"),
        source_span,
    )
    // The expression said back rather than quoted from the file: `expr_text`
    // renders the HIR, so the label states what the compiler READ. See
    // [`crate::display`].
    .with_label(
        source_span,
        format!(
            "`{}` is {actual_display}",
            crate::display::expr_text(source_value)
        ),
        frame,
    );

    // The half a reader had to go and find. It names the shape and the property
    // in the vocabulary the PROGRAM writes — bare names — and not in the
    // document's CURIEs (`shop:Order declares shop:total`, which the
    // hand-written target used): a resolved IRI is all that reaches here, and
    // the bare name is the word the author typed on the line above anyway.
    if let Some(at) = declared_at {
        d = d.with_document_label(
            at.span,
            format!("`{}` declares `{property}` as {expected_display}", at.shape),
            at.document,
        );
    }

    if let Some(help) = expected.and_then(|e| repair_for(cx, actual, e, source_value)) {
        d = d.with_help(help);
    }

    Err(cx.raise(d))
}

/// What to do about a value of the wrong type, when the compiler can see a way.
///
/// Two suggestions, both of which `errors/wrong-type`'s hand-written target
/// asked for and neither of which existed — the artefact said *«`Purchase.amount`
/// is Float. If `reference` really holds the number,
/// `parse.float(Purchase.reference)` converts it.»* and blessing the program
/// deleted the only description of them.
///
/// 1. **A column of the EXPECTED type on the same row.** The commonest cause of
///    this error is reaching for the wrong column of the right row, and the
///    right one is already in the schema the checker is holding.
/// 2. **A stdlib function from the actual type to the expected one.** The
///    catalogue is DATA, so this is a search rather than a table of special
///    cases: a row taking exactly one `actual` and returning `expected`.
///
/// `None` when neither fires, which is when the compiler has nothing to add to
/// what the labels already showed.
///
/// # The candidates are SORTED, and that is not tidiness
///
/// `FunctionRegistry::iter` walks a `HashMap`, so its order is the process's
/// hash seed. An unsorted `first` would put a different function in the message
/// on different runs of the same compiler — and this text is committed as a
/// conformance artefact, so it would be a golden that fails at random.
/// `crates/fossil-shex/examples/declaration_order.rs` measured this exact class
/// of bug once already: six parses, six orders.
fn repair_for<'db>(
    cx: &Expr<'db>,
    actual: Ty<'db>,
    expected: Ty<'db>,
    source_value: &HirExpr,
) -> Option<String> {
    let db = cx.db();
    let mut parts: Vec<String> = Vec::new();
    let expected_display = render_ty_kind(db, expected.kind(db));

    // (1) The row the value was read from, and the columns of it that WOULD
    //     satisfy the constraint. Only for a reference: a literal or a call has
    //     no row to look across.
    let (row, binding, written) = match source_value {
        HirExpr::ColumnRef { binding, column } => (
            cx.rows.as_ref().and_then(|r| r.row_of(binding)),
            Some(binding.as_str()),
            Some(column.as_str()),
        ),
        HirExpr::FieldRef(name) => (cx.flat, None, Some(name.as_str())),
        _ => (None, None, None),
    };
    if let (Some(row), Some(written)) = (row, written)
        && let TyKind::Record(rec) = row.kind(db)
    {
        let fits: Vec<String> = rec
            .fields(db)
            .iter()
            .filter(|f| f.ty == expected && f.name != written)
            .map(|f| {
                binding.map_or_else(|| format!("`{}`", f.name), |b| format!("`{b}.{}`", f.name))
            })
            .collect();
        // Two named and the rest counted. Naming ten columns is not a
        // suggestion, and naming two of ten silently is a lie about the row.
        if let Some((first, rest)) = fits.split_first() {
            let named = match rest.split_first() {
                None => first.clone(),
                Some((second, [])) => format!("{first} and {second}"),
                Some((second, more)) => format!("{first}, {second} and {} others", more.len()),
            };
            let verb = if fits.len() == 1 { "is" } else { "are" };
            parts.push(format!("{named} {verb} {expected_display}."));
        }
    }

    // (2) A conversion, spelled as the call the author would write.
    let is_ty =
        |tag: Option<crate::stdlib::ScalarTy>, t: Ty<'db>| tag.is_some_and(|s| s.to_ty(db) == t);
    let mut conversions: Vec<&crate::stdlib::RegistryEntry> = crate::stdlib::stdlib()
        .iter()
        .filter(|e| {
            is_ty(e.sig.ret.scalar(), expected)
                && matches!(e.sig.params.as_slice(), [only] if is_ty(only.ty.scalar(), actual))
        })
        .collect();
    // **The one NAMED after the type wins**, then alphabetical. `parse.decimal`
    // and `parse.float` both answer String → Float — the catalogue has no
    // Decimal type in the MVP lattice, so `decimal` returns a Float by
    // compromise and says so in its own row — and sorting on the name alone
    // put `decimal` in front of an author who asked for a Float. The tiebreak
    // is what a reader would reach for; the alphabetical order under it is what
    // keeps the answer the same on every run.
    let wanted = expected_display.to_ascii_lowercase();
    conversions.sort_by(|a, b| (a.member != wanted, &a.name).cmp(&(b.member != wanted, &b.name)));
    if let Some(convert) = conversions.first() {
        let text = crate::display::expr_text(source_value);
        let subject = written.map_or_else(|| format!("`{text}`"), |w| format!("`{w}`"));
        parts.push(format!(
            "If {subject} really holds the value, `{}({text})` converts it.",
            convert.name
        ));
    }

    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The cardinality a shape declares, in the words the `help:` uses.
///
/// `Occurs` is a `(min, max)` pair and every diagnostic that reads it wants a
/// phrase; rendering it at each site is how two of them come to disagree about
/// what `(1, None)` is called.
fn render_occurs(o: Occurs) -> String {
    match (o.min, o.max) {
        (1, Some(1)) => "exactly one".to_string(),
        (0, Some(1)) => "at most one".to_string(),
        (n, None) => format!("{n} or more"),
        (lo, Some(hi)) if lo == hi => format!("exactly {lo}"),
        (lo, Some(hi)) => format!("between {lo} and {hi}"),
    }
}

/// The cardinality `shape` declares for `iri` — [`Occurs::ONE`] when the shape
/// does not declare it, which is the default a document that says nothing means
/// and the only value a MISSING required predicate can have reached this by.
fn shape_occurs(shape: &ResolvedShape<'_>, iri: &SmolStr) -> Occurs {
    shape
        .constraint_for(iri.as_str())
        .map_or(Occurs::ONE, |c| c.occurs)
}

/// Where a shape document declares the constraint a value failed.
///
/// The three fields are the three things a label needs and they come from three
/// places — the range from the decoder ([`crate::shapes::ShapeConstraint::span`]),
/// the document from the binding that brought the shape in
/// ([`crate::shapes::ResolvedShape::document`]), and the shape's name from the
/// mapping header. Bundled because a function taking them loose can be handed
/// them in the wrong order and still compile.
#[derive(Debug, Clone)]
pub struct Declared {
    /// The range in the DOCUMENT, file-absolute in that document's text.
    pub span: Span,
    /// The document, as the program named it.
    pub document: SmolStr,
    /// The shape's name as the PROGRAM bound it — `Order`, not `shop:Order`.
    pub shape: SmolStr,
}

// `expr_contains_free_field_refs`, `rewrite_field_refs_to_row_dot` and
// `render_leaf_expr_text` lived here, and all three existed for ONE caller:
// the implicit-closure fast-path in `Checker::check`. They are named as dead in
// `grammar.bnf`'s own tombstone for `FieldRef` — «existed ONLY because a
// predicate did not name its row» — and the walk they performed answers a
// question the language stopped asking: which sub-expressions read the
// anonymous current row. Every reference is qualified now (`User.name`), so a
// closure has nothing to capture implicitly.

// `op_text` and `un_op_text` were here, spelling an operator for a diagnostic.
// They are a fact about the operator and not about the checker, and the census
// needs the same spelling for the same reason — so they live beside the enums,
// in `crate::display`, and there is one table rather than two that agree until
// one of them moves.

/// Recursive subtyping — four rules: S-Refl, S-IntFlt, S-TmplIri, S-SeqCov,
/// plus an error-taint escape so one mismatch does not cascade.
/// Direct enum dispatch — NO `Box<dyn>`, NO trait objects.
pub(crate) fn subtypes<'db>(
    db: &'db dyn fossil_base::Db,
    actual: Ty<'db>,
    expected: Ty<'db>,
) -> bool {
    // S-Refl: every type subtypes itself (pointer equality after interning).
    if actual == expected {
        return true;
    }
    // An Error type is compatible with anything (taint already emitted a
    // diagnostic; do not cascade).
    if matches!(actual.kind(db), TyKind::Error(_)) || matches!(expected.kind(db), TyKind::Error(_))
    {
        return true;
    }
    // S-IntFlt and S-TmplIri both answer `true`, and they are not one arm: each
    // is a named subtyping rule with its own reason, and the reason is what the
    // arm above it carries. Merged, the two rules would share one comment and
    // neither would be findable by name.
    #[allow(clippy::match_same_arms)]
    match (actual.kind(db), expected.kind(db)) {
        // S-IntFlt: Integer <: Float.
        (TyKind::Primitive(Primitive::Integer), TyKind::Primitive(Primitive::Float)) => true,
        // S-RefSub: a reference to a narrower SET of shapes satisfies a slot
        // that accepts a wider one. `@<A>` where the shape declares
        // `@<A> OR @<B>` — a member type is assignable to the union, as in
        // `GraphQL`, and `sh:or` reads the same way.
        //
        // It replaces S-TmplIri, whose whole content was undoing the
        // distinction between `IriTemplate` and `Iri` — a rule that existed
        // because the type did.
        (TyKind::Ref(a), TyKind::Ref(e)) => a.iter().all(|s| e.contains(s)),
        // S-SeqCov: Seq<τ> <: Seq<τ'> when τ <: τ'.
        (TyKind::Seq(a_inner), TyKind::Seq(e_inner)) => subtypes(db, *a_inner, *e_inner),
        _ => false,
    }
}

/// Where a diagnostic about an expression points, and in which frame.
///
/// A mapping body has one span per expression, mapping-relative, and a host
/// rebases them. A pipeline has ONE span for the whole `name := …` item —
/// [`crate::lower::HirSourcePipe::span`] says why — and it is file-absolute
/// already, because it is read off the `SOURCE_DEF` node.
///
/// The distinction is not cosmetic. Every pipeline diagnostic in the tree was
/// emitted from inside `typecheck_mapping`, which means
/// [`crate::spans::rebase_to_file`] shifted it by the mapping's start offset —
/// so a message about a `where` on line 5 underlined the mapping body on line 9.
/// `SpanFrame` exists for exactly this and nothing was using it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SpanSource<'db> {
    /// Per-expression spans, mapping-relative.
    Table(Spans<'db>),
    /// One span for every expression, already file-absolute.
    At(Span),
}

/// Everything typing an EXPRESSION needs, and nothing a mapping body needs.
///
/// The split is the fix for a class of defect rather than one instance. The
/// checker was mapping-shaped — it held a `MappingLoc` and the per-mapping span
/// table — so the only way to type an expression was to be inside a mapping.
/// A source pipeline's predicate is not, and so it was walked for the column
/// NAMES it mentions (`crate::infer`'s hand-written `check_refs`) and never
/// typed: `Row.celsius > "abc"` passed clean two lines above a
/// `parse.float(Row.celsius)` refused for the same mismatch.
///
/// What an expression actually needs is the file whose declarations it names,
/// the rows in scope, somewhere to point, and somewhere to put errors. What a
/// BODY adds is the target shape and its predicate table. Those are two
/// different things and they are two structs.
// `missing_debug_implementations`: holds `&dyn fossil_base::Db`, which is not
// `Debug`.
#[allow(missing_debug_implementations)]
pub struct Expr<'db> {
    pub(crate) db: &'db dyn fossil_base::Db,
    /// The file whose bindings and type declarations this expression names —
    /// what `synth_edge` resolves a target against. It was reached through the
    /// mapping, and it is a FILE-level question: `lookup_type` and
    /// `subject_templates` are both keyed by file.
    pub(crate) file: SourceFile,
    /// The rows in scope, each under the binding that introduced it.
    pub(crate) rows: Option<Rows<'db>>,
    /// [`Self::rows`] flattened — what a BARE name resolves against. A
    /// qualified reference must not use it: flattening is what loses the
    /// binding.
    pub(crate) flat: Option<Ty<'db>>,
    /// The name of the relation these rows came from, for provenance: a
    /// mapping's `from` binding, or the binding at the head of a pipe.
    pub(crate) relation: SmolStr,
    pub(crate) spans: SpanSource<'db>,
    pub(crate) entries: Vec<ExprTypeEntry<'db>>,
    /// First type error encountered (if any). Every error also pushes a
    /// diagnostic, so any `Some(eg)` here implies >= 1 emitted `Diagnostic`.
    pub(crate) first_error: Option<ErrorGuaranteed>,
    /// The shapes something above is expecting a reference to, if any.
    ///
    /// It exists because an interpolation's type is not a property of the
    /// expression: `"…{u.id}"` is the identity of a node where a node is
    /// wanted and a string anywhere else. That is the rule read literally, and
    /// it is what `TyKind::IriTemplate` and a subtyping rule were standing in
    /// for.
    pub(crate) expected_ref: Option<Vec<SmolStr>>,
}

impl<'db> Expr<'db> {
    /// An expression checker over `rows`, pointing everything at one span.
    ///
    /// The pipeline constructor. `at` is file-absolute, and
    /// [`Self::error`] marks every diagnostic accordingly — see [`SpanSource`].
    pub(crate) fn over_relation(
        db: &'db dyn fossil_base::Db,
        file: SourceFile,
        relation: SmolStr,
        rows: Rows<'db>,
        at: Span,
    ) -> Self {
        Self {
            db,
            file,
            flat: rows.flat(db),
            rows: Some(rows),
            relation,
            spans: SpanSource::At(at),
            entries: Vec::new(),
            first_error: None,
            expected_ref: None,
        }
    }

    const fn db(&self) -> &'db dyn fossil_base::Db {
        self.db
    }

    fn span_of(&self, expr_id: ExprId) -> Span {
        match self.spans {
            SpanSource::Table(t) => t.get(self.db, expr_id).unwrap_or(Span { start: 0, end: 0 }),
            SpanSource::At(span) => span,
        }
    }

    /// What this checker's spans are measured against — see [`SpanSource`].
    ///
    /// A pipeline's span is file-absolute and a body's is mapping-relative, and
    /// a [`SpanLabel`] carries its own frame, so an emitter that attaches one
    /// has to spell out the same answer [`Self::raise`] applies to the
    /// diagnostic. Both read it here rather than each deciding again.
    const fn frame(&self) -> SpanFrame {
        match self.spans {
            SpanSource::At(_) => SpanFrame::FileAbsolute,
            SpanSource::Table(_) => SpanFrame::MappingRelative,
        }
    }

    /// Raise a built [`Diagnostic`], in the frame this checker's spans are in.
    ///
    /// THE emission point, and it is one so that the frame is decided once. It
    /// was `delay_span_bug` at twenty-seven call sites, each defaulting to
    /// `SpanFrame::MappingRelative` — correct for a body and silently wrong for
    /// a pipeline, whose span is already file-absolute.
    ///
    /// [`Self::error`] is this over a bare message. The split is for the
    /// emitters that attach a label or a `help:` — they need the builder, and
    /// routing them around this would put the frame decision back at the call
    /// site, which is the bug the paragraph above records.
    fn raise(&mut self, d: Diagnostic) -> ErrorGuaranteed {
        let d = if matches!(self.frame(), SpanFrame::FileAbsolute) {
            d.file_absolute()
        } else {
            d
        };
        let eg = fossil_base::raise(self.db, d);
        self.record_error(eg);
        eg
    }

    /// [`Self::raise`] over a message and a span, with no label.
    fn error(&mut self, span: Span, message: impl Into<String>) -> ErrorGuaranteed {
        self.raise(Diagnostic::new(Severity::Error, message, span))
    }

    /// [`Self::error`] at the span of an expression.
    fn error_at(&mut self, expr_id: ExprId, message: impl Into<String>) -> ErrorGuaranteed {
        self.error(self.span_of(expr_id), message)
    }

    const fn record_error(&mut self, eg: ErrorGuaranteed) {
        if self.first_error.is_none() {
            self.first_error = Some(eg);
        }
    }
}

// S-Opt and S-OptCov were two arms here, and `is_optional` was the predicate
// the cardinality check asked. All three are gone with `TyKind::Optional`,
// which nothing outside a test ever constructed: the language has no `T?` (it
// is not in `grammar.bnf`), `expected_value_ty` emits `Primitive` or `Iri`, and
// `record_from_inferred` types every descriptor column bare. A rule over a type
// that cannot exist is not a rule.

// `demands_one_or_more` lived here as a five-armed match over a cardinality
// enum that could hold two spellings of one fact (`Exact(3)` answered `true`
// where `Range { min: 3, max: Some(3) }` answered `false`). `Occurs` is a
// `(min, max)` pair and carries the answer as `Occurs::demands_one_or_more`.

impl<'db> Expr<'db> {
    /// Inference-mode descent over a leaf [`HirExpr`].
    ///
    /// `HirExpr` is recursive now (`Call` / `Ternary` / `BinOp` /
    /// `Interpolation` all carry sub-expressions), so `synth` dispatches into
    /// the arms below. It had one mode transition — the `synthesize_closure`
    /// hook — and that hook is deleted: there is no checking-mode entry left in
    /// this impl, because the only caller with an expectation to give is
    /// [`Checker::check_property`], which applies it itself.
    pub fn synth(&mut self, expr_id: ExprId, e: &HirExpr) -> Option<Ty<'db>> {
        let (ty, kind) = self.synth_ty(expr_id, e)?;
        let span = self.span_of(expr_id);
        self.entries.push(ExprTypeEntry {
            expr_id,
            ty,
            provenance: Provenance { span, kind },
        });
        Some(ty)
    }

    /// The type and its provenance, with nothing recorded.
    ///
    /// [`Self::synth`] is this plus the arena entry. The split exists because a
    /// call's arguments share the call's `expr_id` — recording them would put
    /// several entries under one id and hover would read whichever came first.
    fn synth_ty(&mut self, expr_id: ExprId, e: &HirExpr) -> Option<(Ty<'db>, ProvenanceKind)> {
        let db = self.db;
        let out = match e {
            HirExpr::StringLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::String)),
                ProvenanceKind::Literal,
            ),
            // T-Interp: the holes are typed like the expressions they are, and
            // the interpolation's own type comes from where it sits — IRI under
            // `iri =`, a string anywhere else.
            HirExpr::Interpolation(parts) => {
                for part in parts {
                    if let InterpolationPart::Hole(e) = part {
                        self.synth_ty(expr_id, e);
                    }
                }
                // An interpolation is a string unless something is expecting a
                // reference, in which case it IS that reference: the holes are
                // filled per row and the result is the identity of a node. That
                // is bidirectional checking doing what a `TyKind::IriTemplate`
                // and a subtyping rule were standing in for.
                let ty = self.expected_ref.clone().map_or_else(
                    || Ty::new(db, TyKind::Primitive(Primitive::String)),
                    |shapes| Ty::reference(db, shapes),
                );
                (ty, ProvenanceKind::Literal)
            }
            // T-Column: the qualified spelling. Same resolution as T-Field,
            // plus the check the anonymous form could never make — that the
            // name on the left is a row this mapping actually reads. That check
            // is the point of qualifying.
            //
            // **It was an equality against the `from` name, and that contradicted
            // the grammar.** `SourceDef` states the consequence of the split
            // between the two names — *«a mapping body writes `User.name` and never
            // `Adults.name`, even when it draws `from Adults`»* — so the name on
            // the left is a BINDING and the name after `from` is a RELATION, and
            // the two coincide only when the relation is a binding that reads a
            // file. Nine of the twenty-three conformance programs are written as
            // the grammar says and were rejected by this equality.
            //
            // It is a lookup over the scope, and the scope is what a `join` puts
            // two rows in: `Purchase.amount` and `User.email` land on different
            // entries, so two columns called `id` are two columns and not one.
            HirExpr::ColumnRef { binding, column } => {
                let Some(row_scope) = self.rows.as_ref() else {
                    // No `from` clause resolved at all — the header did not
                    // lower. Whatever refused it has already spoken; a second
                    // message per column would bury it.
                    return None;
                };
                if !row_scope.has(binding) {
                    let source_name = self.relation.clone();
                    self.error_at(
                        expr_id,
                        format!(
                            "`{binding}.{column}` reads a row this mapping does not \
                             have; it maps `{source_name}`. Name that row, or bring \
                             `{binding}` in."
                        ),
                    );
                    return None;
                }
                let ty = self.lookup_column(expr_id, binding, column)?;
                (
                    ty,
                    ProvenanceKind::InputDescriptor {
                        // The BINDING, not the `from` name: the column comes off
                        // `Contact`, and `Reachable` is the relation that carries
                        // it. Hover shows where a value came from, and after a
                        // join the `from` name is not where any of them came from.
                        source_name: binding.clone(),
                        column: column.clone(),
                    },
                )
            }
            // T-Field: resolve against the source row.
            HirExpr::FieldRef(name) => {
                let ty = self.lookup_field(expr_id, name)?;
                let source_name = self.relation.clone();
                (
                    ty,
                    ProvenanceKind::InputDescriptor {
                        source_name,
                        column: name.clone(),
                    },
                )
            }
            // T-App: resolve the name in the stdlib catalog, check the
            // arguments against the declared parameter types, and take the
            // declared return type. Every failure here is a diagnostic and an
            // `Error` type — never a silently dropped property.
            HirExpr::Call { func, args } => (
                self.synth_call(expr_id, func, args),
                ProvenanceKind::FnResult { name: func.clone() },
            ),
            // T-Edge: `Person(User.email)` is an IRI — the identity of a
            // `Person`, built from this row. This is the ONLY per-row producer
            // of `TyKind::Iri` the language has, which is what makes the
            // `expected_value_ty` rule for a shape-ranged predicate satisfiable
            // at all (`shapes.rs`, case 2: «the only thing a reference can be is
            // an IRI»). Before it, that rule could be met by nothing.
            HirExpr::Edge { target, args } => (
                self.synth_edge(expr_id, target, args),
                ProvenanceKind::FnResult {
                    name: target.clone(),
                },
            ),
            HirExpr::IntLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::Integer)),
                ProvenanceKind::Literal,
            ),
            HirExpr::FloatLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::Float)),
                ProvenanceKind::Literal,
            ),
            // `null` is a value of no type but its own — see
            // [`crate::ty::TyKind::Null`].
            HirExpr::NullLit => (Ty::new(db, TyKind::Null), ProvenanceKind::Literal),
            HirExpr::BoolLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::Bool)),
                ProvenanceKind::Literal,
            ),
            // T-Unary: `-` keeps the operand's numeric type, `not` is Bool to
            // Bool. Neither widens — see [`Self::synth_unary`].
            HirExpr::UnaryOp { op, operand } => (
                self.synth_unary(expr_id, *op, operand)?,
                ProvenanceKind::BinaryOp {
                    op: SmolStr::new_static(un_op_text(*op)),
                },
            ),
            // T-Comp / T-And: both sides must agree, and the result is Bool
            // whether or not the operands could be typed.
            HirExpr::BinOp { op, lhs, rhs } => (
                self.synth_binop(expr_id, *op, lhs, rhs)?,
                ProvenanceKind::BinaryOp {
                    op: SmolStr::new_static(op_text(*op)),
                },
            ),
            HirExpr::Ternary {
                cond,
                then,
                otherwise,
            } => (
                self.synth_ternary(expr_id, cond, then, otherwise)?,
                ProvenanceKind::BinaryOp {
                    op: SmolStr::new_static("?:"),
                },
            ),
        };
        Some(out)
    }

    // `Checker::check` — checking mode — lived here, and its five callers were
    // five tests. `typecheck_mapping` never called it: `check_property` runs its
    // own `synth` + `compatible` because it has to resolve the predicate BEFORE
    // the synth (an interpolation's type depends on what is expected of it), and
    // that ordering is what a generic `check(expected)` could not express. What
    // it carried that nothing else did was the implicit-closure fast-path, and
    // that entry is dead: it fired on `TyKind::Fn`, which nothing constructs
    // because the user declares no functions, over
    // `expr_contains_free_field_refs`, which asked which
    // sub-expressions read the anonymous row the language no longer has.

    /// Resolve a `.field` access against the source row.
    ///
    /// Returns `None` (synthesising no type, preserving the Phase 2 behaviour)
    /// when there is no source row — this keeps `hello.fossil`
    /// (which has a `.name` `FieldRef` and no descriptor behind its source) free
    /// of spurious errors (walking-skeleton invariant). When the row IS known but
    /// the column is missing, emits a did-you-mean diagnostic (SC#1) and
    /// returns an `Error` type.
    pub fn lookup_field(&mut self, expr_id: ExprId, name: &str) -> Option<Ty<'db>> {
        let db = self.db;
        let row = self.flat?; // no source row → no forward propagation

        let TyKind::Record(rec) = row.kind(db) else {
            let eg = self.error_at(
                expr_id,
                format!("internal: source row is not a Record for field `{name}`"),
            );
            return Some(Ty::new(db, TyKind::Error(eg)));
        };

        if let Some(field) = rec.fields(db).iter().find(|f| f.name == name) {
            return Some(field.ty);
        }

        // Miss → did-you-mean over the declared columns. `None` for the
        // binding is the spelling and not the absence of one: a bare name
        // resolved against the flat row, and `refuse_column` names the
        // relation anyway.
        let candidates: Vec<&str> = rec.fields(db).iter().map(|f| f.name.as_str()).collect();
        let eg = self.refuse_column(expr_id, None, name, &candidates);
        Some(Ty::new(db, TyKind::Error(eg)))
    }

    /// «`nmae` is not a field of `User`», with the fields it DOES have in a
    /// label at the binding that answers for them, and the near miss as `help:`.
    ///
    /// One emitter for the bare spelling and the qualified one. They had a
    /// sentence each — `unknown column `x`` and `unknown column `x` on `Y`` —
    /// which is two statements of one fact, and the qualified one was about to
    /// grow a label the other did not have.
    ///
    /// # The repair is a field, not a clause
    ///
    /// It used to be appended to the message (`— did you mean `name`?`) and
    /// then pulled back OUT of it by a `find("did you mean")` over the message
    /// text: `fossil-cli`'s `extract_did_you_mean` and this crate's conformance
    /// harness each did their own. Both still run, and neither has anything to
    /// find now, because [`fossil_base::Diagnostic::help`] carries it.
    ///
    /// # The caret is on the NAME, and so is the quick-fix
    ///
    /// `name = User.nmae` underlines `nmae`, not `User.nmae`, and the same
    /// range goes into [`fossil_base::Diagnostic::did_you_mean`] — the
    /// structured `(wrong_span, replacement)` pair `fossil_ide::code_action`
    /// turns into a one-edit `WorkspaceEdit`. **This is that field's first
    /// producer.** Every caller of `with_did_you_mean` in the workspace was a
    /// test, so the editor's did-you-mean action could not fire on a real
    /// diagnostic, and it passed its own tests throughout.
    ///
    /// Both wanted the same thing and neither could have it: the finest span
    /// the compiler recorded was a whole right-hand side. See
    /// [`crate::body::HirBody::ref_spans`], and note the fallback — a reference
    /// the walk did not record leaves the caret where it has always been.
    fn refuse_column(
        &mut self,
        expr_id: ExprId,
        binding: Option<&str>,
        column: &str,
        candidates: &[&str],
    ) -> ErrorGuaranteed {
        let db = self.db;
        let rhs = self.span_of(expr_id);
        let span = self.ref_span(expr_id, binding, column).unwrap_or(rhs);
        let frame = self.frame();
        // The relation is named whether or not the SPELLING named it: a bare
        // `nmae` resolves against the flat row, and the flat row came from
        // somewhere.
        let relation = binding.unwrap_or(self.relation.as_str()).to_string();
        let mut d = Diagnostic::new(
            Severity::Error,
            format!("`{column}` is not a field of `{relation}`"),
            span,
        )
        .with_label(span, "here", frame);
        // The binding that introduced the row is where the list of fields is
        // answerable, and it is usually not the line being blamed — `from
        // Adults` puts `User` in scope through a pipeline written elsewhere.
        // Its span is FILE-absolute while this diagnostic is mapping-relative,
        // which is the case `SpanLabel`'s own frame exists for.
        if let Some(at) = def_map(db, self.file).lookup_source_span(db, &relation) {
            d = d.with_label(
                at,
                format!("`{relation}` has the fields {}", candidates.join(", ")),
                SpanFrame::FileAbsolute,
            );
        }
        if let Some(s) = did_you_mean(column, candidates.iter().copied()) {
            d = d.with_help(format!("did you mean `{s}`?"));
            // The quick-fix replaces `span` with `s`, so it is only offered
            // when `span` IS the name. Falling back to the right-hand side
            // would generate an edit that deletes `User.` along with the typo.
            if span != rhs {
                d = d.with_did_you_mean(span, s);
            }
        }
        self.raise(d)
    }

    /// [`crate::spans::Spans::ref_span`], for the checker's own frame.
    ///
    /// `None` for a pipeline: [`SpanSource::At`] is one span for the whole
    /// expression by construction — `crate::infer` checks a `where(…)` against
    /// a span it was handed, not against a per-mapping table — so there is no
    /// finer range to find and the caller keeps the one it has.
    fn ref_span(&self, expr_id: ExprId, binding: Option<&str>, name: &str) -> Option<Span> {
        match self.spans {
            SpanSource::Table(t) => t.ref_span(self.db, expr_id, binding, name),
            SpanSource::At(_) => None,
        }
    }

    /// Resolve `Contact.email` against the ROW `Contact` contributes, and never
    /// against the flattening of the whole scope.
    ///
    /// That distinction is the entire point of the qualified spelling, and it is
    /// the trap this function exists to avoid: after `Purchase.join(User, …)`
    /// the flat row holds two columns called `id`, and a lookup by bare name
    /// finds the first — so `User.id` resolving through [`Self::lookup_field`]
    /// would type against `Purchase.id` and compile a program that writes the
    /// other row's column. Silent, and worse than the refusal it replaces.
    ///
    /// Returns `None` — synthesising no type, exactly as [`Self::lookup_field`]
    /// does — when the binding is in scope but its source declares no schema.
    /// The caller has already established the binding IS in scope
    /// ([`crate::ty::Rows::has`]); an absent row here means "no
    /// descriptor", which is not an error.
    fn lookup_column(&mut self, expr_id: ExprId, binding: &str, column: &str) -> Option<Ty<'db>> {
        let db = self.db;
        let row = self.rows.as_ref()?.row_of(binding)?;

        let TyKind::Record(rec) = row.kind(db) else {
            let eg = self.error_at(
                expr_id,
                format!("internal: the row `{binding}` contributes is not a Record"),
            );
            return Some(Ty::new(db, TyKind::Error(eg)));
        };

        if let Some(field) = rec.fields(db).iter().find(|f| f.name == column) {
            return Some(field.ty);
        }

        // Miss → did-you-mean over THAT binding's columns. Over the whole scope
        // it would suggest a column of the other side of a join, which is a
        // suggestion that does not compile.
        let candidates: Vec<&str> = rec.fields(db).iter().map(|f| f.name.as_str()).collect();
        let eg = self.refuse_column(expr_id, Some(binding), column, &candidates);
        Some(Ty::new(db, TyKind::Error(eg)))
    }

    /// T-Edge: type a `Person(User.email)` against the target type's identity.
    ///
    /// Always `Iri` — «the only thing a reference to another node can be is an
    /// IRI», which is the rule [`crate::shapes::expected_value_ty`] applies on
    /// the other side. The type is returned even when the constructor is wrong,
    /// because poisoning is done through `record_error` and returning `Error`
    /// here as well would report the same property twice.
    ///
    /// Three things can go wrong and each is a diagnostic with a real span:
    ///
    /// 1. **The name binds no shape.** `type { Person } := io.shex(…)` where the
    ///    document declares fewer shapes than the binding names — `def_map`
    ///    already knows why and carries it as a `ShapeBindError`.
    /// 2. **No mapping in this file produces that type**, so there is no
    ///    template to fill. It is NOT an error for an edge to POINT at a type
    ///    nothing emits — RDF is open-world and ruling 6 of `SURFACE-PLAN.md`
    ///    says so — but it is an error to CONSTRUCT one, because construction
    ///    needs the template.
    /// 3. **The arity is wrong**: the template has N holes and the call passed
    ///    M. Both numbers are named, and so is the mapping that declared the
    ///    template, because a reader who mis-counted needs to see the identity
    ///    they are constructing.
    fn synth_edge(&mut self, expr_id: ExprId, target: &SmolStr, args: &[HirExpr]) -> Ty<'db> {
        let db = self.db;
        // The type of an edge is the SHAPE it reaches, not «an IRI». It was
        // `TyKind::Iri` with `shape_iri` sitting resolved two statements below,
        // and that is why `buyer = Order(…)` against `shop:buyer @shop:Person`
        // compiled clean.
        //
        // The fallbacks below are the paths where the target resolved to
        // nothing, and each has already emitted its own diagnostic. The EMPTY
        // set is deliberate and it is not «no shapes»: `all()` over nothing is
        // vacuously true, so a reference to no shape satisfies every slot, and
        // the property is not blamed a second time for a target the author was
        // already told about. It is the role `TyKind::Error` plays for the rest
        // of the checker, minus the taint — an unresolvable target is the
        // program's mistake and it has been reported.
        let unresolved = || Ty::reference(db, std::iter::empty());
        // The arguments are ordinary expressions in THIS mapping's scope and are
        // typed as such — that is the whole content of «the argument is the
        // hole's finished value». They share the call's `expr_id` for the same
        // reason `synth_call`'s do.
        for a in args {
            self.synth_ty(expr_id, a);
        }

        let file = self.file;
        let Some(shape_iri) = crate::def_map::def_map(db, file).lookup_type(db, target.as_str())
        else {
            self.error_at(
                expr_id,
                format!(
                    "`{target}` names no shape, so there is no identity to build. A `type {{ … }} \
                     := io.shex(…)` binding introduces the name, and the Nth name takes the Nth \
                     shape the document declares."
                ),
            );
            return unresolved();
        };

        // Read lazily, and that is load-bearing: `subject_templates` is
        // file-keyed and depends on every mapping's body, so a mapping that
        // constructs no edge must not read it. See `crate::identity`.
        let Some(template) =
            crate::identity::subject_templates(db, file).for_shape(db, shape_iri.as_str())
        else {
            self.error_at(
                expr_id,
                format!(
                    "no mapping in this program writes a `{target}`, so `{target}(…)` has no \
                     identity template to build from. An edge may POINT at a type nothing here \
                     emits — RDF is open-world — but it is built from the `@subject` of the \
                     mapping that does emit it."
                ),
            );
            return unresolved();
        };

        let arity = template.arity();
        if args.len() != arity {
            let declared = template.mapping_name;
            self.error_at(
                expr_id,
                format!(
                    "`{target}` is built from {arity} value(s) and this passes {}. Its identity \
                     is declared by `{declared}`, whose `@subject` has {arity} hole(s); the Nth \
                     value fills the Nth hole, in order.",
                    args.len()
                ),
            );
        }
        Ty::reference(db, std::iter::once(SmolStr::from(shape_iri.as_str())))
    }

    /// T-App: type a `clean.slug(.name)` against the stdlib catalog.
    ///
    /// Four things can go wrong and all four are diagnostics with an `Error`
    /// type, so the mapping is poisoned and the property is never written from
    /// a value nobody checked: the name is not catalogued (with a did-you-mean
    /// over the catalog), the arity is wrong, an argument does not type, or an
    /// argument's type is not a subtype of the declared parameter's.
    ///
    /// Arguments carry the CALL's `expr_id`: the body arena holds one entry per
    /// property value, so a sub-expression has no id of its own. That makes
    /// every diagnostic inside a call point at the whole call — imprecise, and
    /// honestly so; per-argument spans need the arena to hold sub-expressions.
    fn synth_call(&mut self, expr_id: ExprId, func: &SmolStr, args: &[HirExpr]) -> Ty<'db> {
        let db = self.db;
        let reg = crate::stdlib::stdlib();
        let Some(entry) = reg.lookup(func.as_str()) else {
            // **Dispatch by RECEIVER, not by string.** The name
            // is dotted, so the miss has a shape, and saying WHICH HALF is
            // wrong is the whole difference between this and a string compare:
            // an unknown receiver and an unknown member are different mistakes
            // and used to produce the same sentence.
            let msg = match func.split_once('.') {
                Some((head, member)) if !reg.is_catalogued_head(head) => {
                    let heads = crate::didyoumean::did_you_mean(
                        head,
                        reg.iter().filter_map(|e| e.name.split('.').next()),
                    );
                    heads.map_or_else(
                        || {
                            format!(
                                "`{head}` is not a namespace or a type fossil knows, so it has \
                                 no member `{member}`"
                            )
                        },
                        |s| format!("`{head}` has no members — did you mean `{s}.{member}`?"),
                    )
                }
                Some((head, member)) => {
                    // The receiver EXISTS and the member does not. Suggest from
                    // that receiver's members only — the whole point of
                    // `members_of` is that the candidate set is the receiver's,
                    // not the catalogue's.
                    let (recv, _) = crate::stdlib::split_receiver(func.as_str());
                    let siblings = reg.members_of(recv).map(|e| e.member.as_str());
                    crate::didyoumean::did_you_mean(member, siblings).map_or_else(
                        || format!("`{head}` has no member `{member}`"),
                        |s| {
                            format!(
                                "`{head}` has no member `{member}` — did you mean `{head}.{s}`?"
                            )
                        },
                    )
                }
                None => format!("unknown function `{func}`"),
            };
            let eg = self.error_at(expr_id, msg);
            return Ty::new(db, TyKind::Error(eg));
        };

        let params = &entry.sig.params;
        if args.len() != params.len() {
            let eg = self.error_at(
                expr_id,
                format!(
                    "`{func}` takes {} argument{}, but {} {} given",
                    params.len(),
                    if params.len() == 1 { "" } else { "s" },
                    args.len(),
                    if args.len() == 1 { "was" } else { "were" },
                ),
            );
            return Ty::new(db, TyKind::Error(eg));
        }

        for (i, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
            // A relation and a condition are not scalars, and neither is
            // written as a value in the surface: a `seq/` row is reached by
            // `Row.where(…)`, whose receiver and predicate `crate::lower`
            // lifts into a `HirSourcePipe` rather than into `Call` arguments.
            // They are checked where the pipeline is — `crate::infer` — with
            // the rows of THAT stage in scope, which is a thing this frame does
            // not have.
            let Some(scalar) = param.ty.scalar() else {
                continue;
            };
            let expected = scalar.to_ty(db);
            // An argument with no type is not an error: without a source
            // descriptor a `.field` synthesises nothing, and the rest of the
            // checker lets that through rather than inventing one. An argument
            // it cannot see is an argument it cannot check — which is what
            // forward propagation is FOR, and F3 is what makes it always
            // present. What it must not do is claim a problem the program does
            // not have.
            let Some((actual, _)) = self.synth_ty(expr_id, arg) else {
                continue;
            };
            if matches!(actual.kind(db), TyKind::Error(_)) {
                return actual;
            }
            if !subtypes(db, actual, expected) {
                // Argument 0 of a `Receiver::Scalar` row IS the receiver — the
                // two spellings both put it there, `str.trim(x)` writing
                // it and `x.trim()` moving it. So a type error on it is a
                // statement about what the value IS, and saying "argument 1"
                // for `x.trim()` would name a position the author never wrote.
                let is_receiver =
                    i == 0 && matches!(entry.recv, crate::stdlib::Receiver::Scalar(_));
                let msg = if is_receiver {
                    format!(
                        "`{}` is a member of {}, and this is {}",
                        entry.member,
                        render_ty_kind(db, expected.kind(db)),
                        render_ty_kind(db, actual.kind(db)),
                    )
                } else {
                    format!(
                        "argument {} of `{func}` expects {}, but this is {}",
                        i + 1,
                        render_ty_kind(db, expected.kind(db)),
                        render_ty_kind(db, actual.kind(db)),
                    )
                };
                let eg = self.error_at(expr_id, msg);
                return Ty::new(db, TyKind::Error(eg));
            }
        }

        entry.sig.ret.scalar().map_or_else(
            // A row with a `Rows` return gives back a RELATION: a verb of the
            // algebra hands on the one it was given, an `io/` constructor makes
            // the first one. Neither is a value.
            //
            // A pipeline is lifted out of `Call` by `crate::lower`, and so is a
            // source binding (on the `io.` prefix), so what reaches here is the
            // relation written where a value belongs — `M.x = io.csv("u.csv")`.
            // That used to type as `xsd:string`, because the three `io/` rows
            // declared `String`; the message does not name verbs any more
            // because the rows are no longer only verbs.
            || {
                Ty::new(
                    db,
                    TyKind::Error(self.error_at(
                        expr_id,
                        format!("`{func}` gives back a relation, which is not a value"),
                    )),
                )
            },
            |s| s.to_ty(db),
        )
    }

    /// T-Comp (`type-system.md` §4.7): `Γ ⊢ a : τ`, `Γ ⊢ b : τ`, τ comparable
    /// ⟹ `Bool`. T-And/T-Or: both sides `Bool` ⟹ `Bool`.
    ///
    /// An operand the checker cannot type — a `.field` with no source
    /// descriptor — is not an error, for the same reason it is not one in a
    /// call: forward propagation is what would give it a type, and F3 is what
    /// makes it always present. For a comparison or a connective the result is
    /// `Bool` regardless, because the operator says so and the operands cannot
    /// change that. For ARITHMETIC it is not: the result type IS the operands',
    /// so an operand with no type leaves the result unknown rather than guessed,
    /// and `Float` is the answer only where the operator forces it.
    ///
    /// # T-Arith, and the two rules it is
    ///
    /// `+`, `-`, `*` and `%` take two numbers and give the WIDER of them —
    /// `Integer` when both are `Integer`, `Float` as soon as either is. That is
    /// `subtypes`'s existing S-IntFlt promotion applied to a result rather than
    /// to a check, so the language has one widening rule and not two.
    ///
    /// **`/` is always `Float`**, and this is a decision rather than a
    /// consequence. The two engines disagree about integer division —
    /// `DuckDB`'s `/` on two `BIGINT`s gives a `DOUBLE`, `DataFusion`'s
    /// `Operator::Divide` on two `Int64`s gives an `Int64` and TRUNCATES — so a
    /// result type read off the operands would mean `7 / 2` is `3` here and
    /// `3.5` there. Typing it `Float` picks the answer that loses nothing, and
    /// `fossil_df::render` casts the left operand so the engine agrees with the
    /// type instead of the type flattering the engine.
    fn synth_binop(
        &mut self,
        expr_id: ExprId,
        op: BinOp,
        lhs: &HirExpr,
        rhs: &HirExpr,
    ) -> Option<Ty<'db>> {
        let db = self.db;
        let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
        let l = self.synth_ty(expr_id, lhs).map(|(t, _)| t);
        let r = self.synth_ty(expr_id, rhs).map(|(t, _)| t);

        for t in [l, r].into_iter().flatten() {
            if let TyKind::Error(_) = t.kind(db) {
                return Some(t);
            }
        }

        match op {
            // T-And / T-Or: each side must BE Bool.
            BinOp::And | BinOp::Or => {
                for (side, ty) in [("left", l), ("right", r)] {
                    let Some(ty) = ty else { continue };
                    if !subtypes(db, ty, bool_ty) {
                        let eg = self.error_at(
                            expr_id,
                            format!(
                                "the {side} side of `{}` must be Bool, but it is {}",
                                op_text(op),
                                render_ty_kind(db, ty.kind(db)),
                            ),
                        );
                        return Some(Ty::new(db, TyKind::Error(eg)));
                    }
                }
            }
            // T-Comp: the two sides must be comparable to each other. Integer
            // and Float compare (the promotion `subtypes` already encodes);
            // a string against a number does not, and that is the mistake
            // worth catching — it is the one a mapping actually makes.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                // `null` compares with anything. The rule is HERE and not in
                // `subtypes`, and that placement is the whole of it: a bottom
                // type that subtyped everything would make `name = null` check
                // against `xsd:string`, which is `TyKind::Optional` returning
                // by the door it left by. Asking whether a column has a value
                // and writing a property from nothing are different questions,
                // and only the first has an answer.
                let against_null = [l, r]
                    .into_iter()
                    .flatten()
                    .any(|t| matches!(t.kind(db), TyKind::Null));
                if let (Some(l), Some(r)) = (l, r)
                    && !against_null
                    && !subtypes(db, l, r)
                    && !subtypes(db, r, l)
                {
                    let eg = self.error_at(
                        expr_id,
                        format!(
                            "cannot compare {} with {} using `{}`",
                            render_ty_kind(db, l.kind(db)),
                            render_ty_kind(db, r.kind(db)),
                            op_text(op),
                        ),
                    );
                    return Some(Ty::new(db, TyKind::Error(eg)));
                }
            }
            // T-Arith. Unlike the two above, the RESULT is the operands' and not
            // the operator's, so this arm returns rather than falling through to
            // `bool_ty`.
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                return self.synth_arith(expr_id, op, l, r);
            }
        }
        Some(bool_ty)
    }

    /// The numeric half of [`Self::synth_binop`]. See its docs for the two
    /// rules; this is where they are applied.
    fn synth_arith(
        &mut self,
        expr_id: ExprId,
        op: BinOp,
        l: Option<Ty<'db>>,
        r: Option<Ty<'db>>,
    ) -> Option<Ty<'db>> {
        let db = self.db;
        let float_ty = Ty::new(db, TyKind::Primitive(Primitive::Float));

        // Three answers and not two. An operand with NO type (no source
        // descriptor yet — not an error, per this function's caller) and an
        // operand of the WRONG type must not collapse into one `None`, because
        // the first leaves the result unknown and the second poisons it.
        let mut widest: Option<bool> = Some(false);
        let mut poisoned: Option<ErrorGuaranteed> = None;
        for (side, ty) in [("left", l), ("right", r)] {
            let Some(ty) = ty else {
                widest = None;
                continue;
            };
            match ty.kind(db) {
                TyKind::Primitive(Primitive::Float) => {
                    if let Some(w) = widest.as_mut() {
                        *w = true;
                    }
                }
                TyKind::Primitive(Primitive::Integer) => {}
                // The mistake this arm exists to catch, and `+` is where it is
                // made: `User.first + " " + User.last` is what someone writes on
                // the first day. It is not string concatenation — `str.concat`
                // is, and one idea does not get two spellings — so the message
                // names the row rather than only refusing.
                kind => {
                    let hint = if matches!(kind, TyKind::Primitive(Primitive::String))
                        && matches!(op, BinOp::Add)
                    {
                        ". Two strings are joined with `str.concat`, not `+`"
                    } else {
                        ""
                    };
                    let eg = self.error_at(
                        expr_id,
                        format!(
                            "the {side} side of `{}` must be a number, but it is {}{hint}",
                            op_text(op),
                            render_ty_kind(db, kind),
                        ),
                    );
                    poisoned = Some(eg);
                }
            }
        }
        if let Some(eg) = poisoned {
            return Some(Ty::new(db, TyKind::Error(eg)));
        }

        // `/` is Float whatever it is given — the one rule that is the
        // operator's rather than the operands'. See `synth_binop`.
        if matches!(op, BinOp::Div) {
            return Some(float_ty);
        }
        match widest {
            // Both operands typed: the wider of the two, which is S-IntFlt
            // applied to a result.
            Some(true) => Some(float_ty),
            Some(false) => Some(Ty::new(db, TyKind::Primitive(Primitive::Integer))),
            // One side has no type at all, so neither has the result — and
            // «no type» is `None`, which is what every caller of `synth_ty`
            // already reads. It was `TyKind::Unknown(fresh_inference())`, and
            // that variant was the OPPOSITE of what this comment asked for:
            // `subtypes` has no arm for it, so it fell to `_ => false` and
            // refused every check it reached instead of standing aside for the
            // shape to answer.
            None => None,
        }
    }

    /// T-Unary: `not` is `Bool → Bool`; `-` keeps its operand's numeric type.
    ///
    /// Neither widens. `-` on an `Integer` is an `Integer` — the negation of a
    /// whole number is a whole number — and promoting it to `Float` would make
    /// `-Row.n` a different type from `Row.n` for no reason the author could see.
    fn synth_unary(&mut self, expr_id: ExprId, op: UnOp, operand: &HirExpr) -> Option<Ty<'db>> {
        let db = self.db;
        let Some(ty) = self.synth_ty(expr_id, operand).map(|(t, _)| t) else {
            // Untypeable operand: the operator still fixes what it CAN. `not`
            // says Bool whatever it is given; `-` cannot, because its result is
            // its operand's type.
            return match op {
                UnOp::Not => Some(Ty::new(db, TyKind::Primitive(Primitive::Bool))),
                UnOp::Neg => None,
            };
        };
        if let TyKind::Error(_) = ty.kind(db) {
            return Some(ty);
        }
        let ok = match op {
            UnOp::Not => matches!(ty.kind(db), TyKind::Primitive(Primitive::Bool)),
            UnOp::Neg => matches!(
                ty.kind(db),
                TyKind::Primitive(Primitive::Integer | Primitive::Float)
            ),
        };
        if !ok {
            let wanted = match op {
                UnOp::Not => "Bool",
                UnOp::Neg => "a number",
            };
            let eg = self.error_at(
                expr_id,
                format!(
                    "`{}` needs {wanted}, but it is given {}",
                    un_op_text(op),
                    render_ty_kind(db, ty.kind(db)),
                ),
            );
            return Some(Ty::new(db, TyKind::Error(eg)));
        }
        Some(ty)
    }

    /// T-Tern (`type-system.md` §4.8): the condition is `Bool`, both branches
    /// have the same type, and that type is the conditional's. **No implicit
    /// coercion** — two branches of different types is the error, not a widening
    /// nobody asked for, because a column whose type depends on the row is a
    /// column no shape can check.
    ///
    /// A branch the checker cannot type yields to the other branch, and if
    /// neither can be typed the conditional cannot either: it is `Unknown`, not
    /// an invented `String`.
    fn synth_ternary(
        &mut self,
        expr_id: ExprId,
        cond: &HirExpr,
        then: &HirExpr,
        otherwise: &HirExpr,
    ) -> Option<Ty<'db>> {
        let db = self.db;
        let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));

        if let Some((c, _)) = self.synth_ty(expr_id, cond) {
            if let TyKind::Error(_) = c.kind(db) {
                return Some(c);
            }
            if !subtypes(db, c, bool_ty) {
                let eg = self.error_at(
                    expr_id,
                    format!(
                        "the condition of `? :` must be Bool, but it is {}",
                        render_ty_kind(db, c.kind(db)),
                    ),
                );
                return Some(Ty::new(db, TyKind::Error(eg)));
            }
        }

        let t = self.synth_ty(expr_id, then).map(|(t, _)| t);
        let o = self.synth_ty(expr_id, otherwise).map(|(t, _)| t);
        for ty in [t, o].into_iter().flatten() {
            if let TyKind::Error(_) = ty.kind(db) {
                return Some(ty);
            }
        }

        match (t, o) {
            (Some(t), Some(o)) => {
                // Same type, or one is a subtype of the other (Integer widens
                // into Float; an interpolation widens into an IRI).
                if subtypes(db, t, o) {
                    Some(o)
                } else if subtypes(db, o, t) {
                    Some(t)
                } else {
                    let eg = self.error_at(expr_id, format!(
                            "the branches of `? :` have different types: {} and {}.                              Both branches must have the same type — fossil does not coerce.",
                            render_ty_kind(db, t.kind(db)),
                            render_ty_kind(db, o.kind(db)),
                        ));
                    Some(Ty::new(db, TyKind::Error(eg)))
                }
            }
            // One branch typed and the other not: the typed one is the best
            // evidence available, and it is evidence, not a guess.
            (Some(t), None) | (None, Some(t)) => Some(t),
            (None, None) => None,
        }
    }

    // `synthesize_closure` lived here — the implicit closure was the ONLY lambda
    // form in the language, and it existed to give `.age >= 18` a row to read
    // `.age` from. `grammar.bnf` names it dead in the same breath as `FieldRef`:
    // a reference is qualified now (`User.age`), so it names its own row and
    // there is nothing left to bind. Its one caller was `Checker::check`, above,
    // and `ProvenanceKind::SynthesizedClosureRendering` is what remains of it —
    // nothing in the workspace constructs that variant any more.
}
