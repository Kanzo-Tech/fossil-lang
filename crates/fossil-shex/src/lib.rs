//! `ShExDescriptor` — wraps a [`shex_ast::Schema`] for Fossil's static
//! target-shape checking (backward shape checking).
//!
//! ## Design summary
//!
//! Fossil's bidirectional checker wants to ask "given
//! target shape `ex:Person`, what is the expected type + cardinality of
//! predicate `p`?". That's a per-shape constraint table. `ShEx` 2.1 lets
//! shapes share triple-expressions via `TripleExpr::Ref(TripleExprLabel)`
//! (forward reference to a named `EachOf`/`OneOf`/`TripleConstraint`
//! declared elsewhere in the same schema).
//!
//! We pre-resolve the AST at construction time — a forward shape reference
//! (`Ref`) left unresolved would be walked as if it denoted nothing:
//! walking a shape body without resolving refs produces "unknown property"
//! false positives. Cycle detection during walk (DFS-visited) — only acyclic
//! shape graphs are supported.
//!
//! ## `OneOf` rejection at shape-lowering time (NOT per-mapping-check)
//!
//! A `OneOf` encounter must produce exactly one deterministic compile error
//! plus a generated Fossil source split-into-N-mappings suggestion. Doing this
//! at per-mapping-check time would duplicate the diagnostic across every
//! consuming mapping. Instead we collect `ShExLoweringError::OneOfRejection`
//! during the construction walk; the typecheck pass emits the diagnostic
//! once, attaching `Diagnostic.suggestion_source` populated by
//! `fossil_hir::render_split_suggestion`.
//!
//! ## The way out is the neutral vocabulary
//!
//! [`ShExDescriptor::to_output_shapes`] lowers the resolved table into
//! [`fossil_graph_schema::OutputShapes`] — shapes, predicates, datatypes,
//! targets, [`fossil_graph_schema::Occurs`], and every lowering error as a
//! [`fossil_graph_schema::Rejection`]. No `shex_ast` or `rudof_iri` type
//! crosses, which is what lets the middle of the compiler read a shape document
//! without linking `ShEx` — the cut `0e6898d` made for `fossil-mir`.
//!
//! [`ShExDescriptor::to_graph_schema`] goes through it rather than beside it, so
//! there is one lowering and not two. The `ShEx`-typed surface
//! ([`ShapeBinding`], [`ResolvedConstraint`], [`ConstraintValue`]) stays for the
//! callers that genuinely want the AST — the INPUT descriptor derives source
//! column types from it.
//!
//! Re-emitting Fossil syntax is NOT among them, and the reasoning that it
//! «only a crate that knows the syntax can do» was wrong: the suggestion needs
//! the consuming mapping's shape name and its `@subject`, which live in the
//! program's CST and not in any shape document. The predicate IRIs in
//! [`fossil_graph_schema::Rejection`] are all a renderer needs from here, so
//! `fossil-hir` renders — and does not depend on this crate at all.
//!
//! ## WASM safety
//!
//! We use ONLY `Schema::from_reader` (byte-stream input) — never
//! `Schema::from_iri` which would pull `reqwest`/`tokio` into the WASM build
//! path. Verified by the original spike and re-verified since.

// `result_large_err`: `ShExLoweringError` carries a `TripleExpr` via
// `OneOfRejection::suggestion_seed::one_of_node`. The two `pub` constructors
// return `Result<Self, ShExLoweringError>` only at the top-level
// `MalformedSchema(String)` failure path — the large variants only flow
// through `lowering_errors()`, never via `Result`. Boxing the entire enum
// would hurt every other consumer.
#![allow(clippy::result_large_err)]

pub mod spans;

use std::collections::{HashMap, HashSet};

use fossil_graph_schema::{
    GraphSchema, Occurs, OutputShapes, Primitive, PropertyConstraint, Rejection, Renames,
    Shape as OutputShape, local_name,
};
use prefixmap::{IriRef, PrefixMap};
use rudof_iri::IriS;
use shex_ast::{
    NodeKind, Schema, ShExParser, Shape, ShapeDecl, ShapeExpr, ShapeExprLabel, TripleExpr,
    TripleExprLabel,
};

#[cfg(test)]
use shex_ast::TripleExprWrapper;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A `ShEx` shape resolved to a flat property table for use by the
/// bidirectional checker.
#[derive(Debug, Clone)]
pub struct ShapeBinding {
    /// The shape's declared IRI (after prefix resolution).
    pub iri: IriS,
    /// Flattened triple constraints — `TripleExpr::Ref`s have been resolved
    /// in-line; `TripleExpr::EachOf` branches have been collapsed; encountered
    /// `OneOf` nodes are NOT included here (their rejection is logged in
    /// [`ShExDescriptor::lowering_errors`]).
    pub constraints: Vec<ResolvedConstraint>,
}

/// One predicate-cardinality pair from a resolved [`ShapeBinding`].
#[derive(Debug, Clone)]
pub struct ResolvedConstraint {
    /// The predicate IRI (after prefix resolution).
    pub predicate: IriS,
    /// The `valueExpr` clause from the `ShEx` `TripleConstraint`, kept opaque
    /// for now — the bidirectional checker narrows this into a `Ty<'db>` against the
    /// `Primitive` lattice.
    pub value_expr: Option<ShapeExpr>,
    /// How many values the predicate may carry, decoded from `ShEx`'s
    /// `(min, max)` integer encoding by [`occurs_from_shex`].
    pub cardinality: Occurs,
}

/// Decode `ShEx`'s `(min, max)` `Option<i32>` encoding into [`Occurs`].
///
/// Per [the ShEx 2.1 spec](https://shex.io/shex-semantics/), `max = -1` means
/// "unbounded" in the JSON form, and `(None, None)` means "exactly 1" (the
/// default). Negative `min`/`max` values other than that one `-1` shouldn't
/// occur in a well-formed schema; they clamp to zero rather than panicking.
///
/// This produces the pair directly. It used to route through a five-variant
/// enum that could spell one cardinality two ways — `Exact(3)` and
/// `Range { min: 3, max: Some(3) }` — and disagree with itself about whether
/// that was single-valued. `fossil-graph-schema`'s
/// `collapse_pins_every_shape_the_old_enum_could_take` pins every case the enum
/// could take, including that unreachable divergence.
#[must_use]
pub fn occurs_from_shex(min: Option<i32>, max: Option<i32>) -> Occurs {
    let to_u32 = |v: i32| u32::try_from(v.max(0)).unwrap_or(0);
    match (min, max) {
        (None, None) => Occurs::ONE,
        (Some(lo), Some(-1)) => Occurs {
            min: to_u32(lo),
            max: None,
        },
        (Some(lo), Some(hi)) => Occurs {
            min: to_u32(lo),
            max: Some(to_u32(hi)),
        },
        (None, Some(hi)) => Occurs {
            min: 1,
            max: Some(to_u32(hi)),
        },
        (Some(lo), None) => Occurs {
            min: to_u32(lo),
            max: Some(to_u32(lo)),
        },
    }
}

/// The Fossil-relevant narrowing of a [`ResolvedConstraint`]'s `ShEx` `valueExpr`.
///
/// `ShEx`'s `valueExpr` is a full `ShapeExpr` lattice; Fossil only needs to know,
/// per property, whether the value is a typed literal (→ a scalar column of a
/// known primitive), an IRI / object reference (→ an edge / IRI-valued column),
/// or something it cannot narrow yet. This is the single decode of that
/// question, shared by the INPUT descriptor (deriving source column types,
/// compile-time) and the OUTPUT bidirectional checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintValue {
    /// A literal constrained to this datatype IRI (e.g.
    /// `http://www.w3.org/2001/XMLSchema#integer`). The consumer maps the IRI
    /// to its own type lattice.
    Datatype(String),
    /// An IRI-valued node (`nodeKind IRI`) or a reference to another shape — an
    /// object property. The value is the referenced subject's IRI. (What turns a
    /// shape-ref into a typed `GraphAr` edge is `edge_targets` filling
    /// [`PropertyConstraint::targets`], which
    /// `fossil_graph_schema::OutputShapes::to_graph_schema` reads; this INPUT
    /// narrowing only needs "is it an IRI".)
    Iri,
    /// Could not be narrowed (`ShapeAnd`/`ShapeOr`/`ShapeNot`/external, or no
    /// `valueExpr` at all). Consumers treat this as an opaque string.
    Unknown,
}

impl ResolvedConstraint {
    /// The local name of the predicate IRI — the field/column name a mapping
    /// references (`http://xmlns.com/foaf/0.1/name` / `foaf:name` → `name`).
    #[must_use]
    pub fn predicate_local_name(&self) -> String {
        local_name(&self.predicate.to_string()).to_string()
    }

    /// Narrow this constraint's `ShEx` `valueExpr` to the Fossil-relevant value
    /// kind. See [`ConstraintValue`].
    #[must_use]
    pub fn value(&self) -> ConstraintValue {
        match &self.value_expr {
            Some(ShapeExpr::NodeConstraint(nc)) => nc.datatype().map_or_else(
                || {
                    if matches!(nc.node_kind(), Some(NodeKind::Iri)) {
                        ConstraintValue::Iri
                    } else {
                        ConstraintValue::Unknown
                    }
                },
                |dt| ConstraintValue::Datatype(iri_ref_to_string(&dt)),
            ),
            // A reference to another shape, or an inline nested shape, is an
            // object property: its value is the referenced subject's IRI.
            Some(ShapeExpr::Ref(_) | ShapeExpr::Shape(_)) => ConstraintValue::Iri,
            _ => ConstraintValue::Unknown,
        }
    }
}

/// Render a [`ShapeExprLabel`] to its IRI string (an edge's destination shape).
/// Full IRIs render verbatim; a rare `Prefixed` form keeps `prefix:local`; a
/// blank-node label renders as its `_:id`.
fn shape_label_iri(label: &ShapeExprLabel) -> String {
    match label {
        ShapeExprLabel::IriRef { value } => match value {
            IriRef::Iri(iri) => iri.to_string(),
            IriRef::Prefixed { prefix, local } => format!("{prefix}:{local}"),
        },
        ShapeExprLabel::BNode { value } => value.to_string(),
        ShapeExprLabel::Start => String::new(),
    }
}

/// The destination shape IRIs of an edge constraint — empty for a literal/opaque
/// property. A single shape `Ref` yields one; a value disjunction `@<A> OR @<B>`
/// (`ShapeOr` of refs) yields all of them, so the canonical model emits one edge
/// type per destination. `ShapeAnd`/`ShapeNot`/`NodeConstraint`/inline `Shape`
/// are not inter-shape edges.
fn edge_targets(value_expr: Option<&ShapeExpr>) -> Vec<String> {
    match value_expr {
        Some(ShapeExpr::Ref(label)) => vec![shape_label_iri(label)],
        Some(ShapeExpr::ShapeOr { shape_exprs }) => shape_exprs
            .iter()
            .filter_map(|w| match &w.se {
                ShapeExpr::Ref(label) => Some(shape_label_iri(label)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The datatype a constraint's `valueExpr` narrows the value to, or `None` when
/// **the document did not narrow it** — `PropertyConstraint::datatype`'s
/// contract.
///
/// `Some(p)` for a typed literal whose datatype IRI is in the XSD lattice;
/// `Some(Primitive::AnyUri)` for a `nodeKind IRI` node, which is an opaque
/// IRI-valued column and not an edge; `None` for everything else, **including an
/// XSD datatype outside the lattice**.
///
/// `None` becomes `Primitive::String` in `OutputShapes::to_graph_schema`, which
/// is exactly what the `Primitive`-valued predecessor of this function returned
/// for those cases — the permissive walking-skeleton column. The `Option` is
/// what lets a consumer tell "the document said `String`" from "the document
/// said nothing", which a bare `Primitive::String` cannot.
fn datatype_of(value_expr: Option<&ShapeExpr>) -> Option<Primitive> {
    match value_expr {
        Some(ShapeExpr::NodeConstraint(nc)) => nc.datatype().map_or_else(
            || matches!(nc.node_kind(), Some(NodeKind::Iri)).then_some(Primitive::AnyUri),
            |dt| Primitive::from_xsd_iri(&iri_ref_to_string(&dt)),
        ),
        _ => None,
    }
}

/// The predicate IRIs a single `OneOf` branch constrains, in order.
///
/// Recurses through `EachOf`. A nested `OneOf` and a `Ref` contribute nothing
/// and are not an error here: the outer `OneOf` is already rejected, and this
/// walk exists only to *name* what the split would be splitting.
fn branch_predicates(expr: &TripleExpr, prefixmap: &PrefixMap, out: &mut Vec<String>) {
    match expr {
        TripleExpr::TripleConstraint { predicate, .. } => {
            if let Some(iri) = resolve_iri_ref(predicate, prefixmap) {
                out.push(iri.to_string());
            }
        }
        TripleExpr::EachOf { expressions, .. } => {
            for w in expressions {
                branch_predicates(&w.te, prefixmap, out);
            }
        }
        TripleExpr::OneOf { .. } | TripleExpr::Ref(_) => {}
    }
}

/// Lower one lowering error to the format-neutral [`Rejection`].
///
/// The one thing that does not survive is the `OneOf` AST node itself, which
/// [`SuggestionSeed`] clones. `Rejection::Disjunction` carries each branch's
/// predicate IRIs instead — enough to name the split with no `ShEx` type in
/// hand, and enough for `fossil_hir::render_split_suggestion` to write the
/// replacement mappings without linking `ShEx`.
fn rejection_of(err: &ShExLoweringError, prefixmap: &PrefixMap) -> Rejection {
    match err {
        ShExLoweringError::OneOfRejection(r) => {
            let disjuncts = match &r.suggestion_seed.one_of_node {
                TripleExpr::OneOf { expressions, .. } => expressions
                    .iter()
                    .map(|w| {
                        let mut out = Vec::new();
                        branch_predicates(&w.te, prefixmap, &mut out);
                        out
                    })
                    .collect(),
                // Unreachable: the seed is only ever built from a `OneOf`.
                _ => Vec::new(),
            };
            Rejection::Disjunction {
                shape_iri: r.shape_iri.to_string(),
                disjuncts,
            }
        }
        ShExLoweringError::CyclicShapeRef { path } => Rejection::CyclicRef { path: path.clone() },
        ShExLoweringError::UnresolvedRef { label, in_shape } => Rejection::UnresolvedRef {
            label: label.clone(),
            in_shape: in_shape.clone(),
        },
        ShExLoweringError::MalformedSchema(m) => Rejection::Malformed(m.clone()),
    }
}

/// Render an [`IriRef`] to its IRI string. Parsed `ShExJ` datatypes are full IRIs
/// (`IriRef::Iri`); a `Prefixed` form (rare in JSON) falls back to `prefix:local`.
fn iri_ref_to_string(iri_ref: &IriRef) -> String {
    match iri_ref {
        IriRef::Iri(iri) => iri.to_string(),
        IriRef::Prefixed { prefix, local } => format!("{prefix}:{local}"),
    }
}

/// Errors discovered while lowering a [`Schema`] into per-shape constraint
/// tables. Surfaced via [`ShExDescriptor::lowering_errors`]; the typecheck
/// pass emits matching `Diagnostic`s.
#[derive(Debug, Clone)]
pub enum ShExLoweringError {
    /// The caller emits one `Diagnostic` carrying the generated split
    /// suggestion in `suggestion_source`.
    OneOfRejection(OneOfRejection),
    /// Only acyclic shape graphs are supported.
    /// `path` is the chain of `TripleExprLabel`s visited along the cycle.
    CyclicShapeRef { path: Vec<String> },
    /// `TripleExpr::Ref(label)` pointing at a label never declared in the
    /// schema.
    UnresolvedRef { label: String, in_shape: String },
    /// Malformed JSON / unsupported top-level structure.
    MalformedSchema(String),
}

/// Carries enough disjunct info to (a) name the constraint in a diagnostic
/// and (b) regenerate the split suggestion text from the live mapping
/// context.
#[derive(Debug, Clone)]
pub struct OneOfRejection {
    /// IRI of the enclosing shape (the shape that declared the `OneOf`).
    pub shape_iri: IriS,
    /// How many disjuncts the `OneOf` carries.
    pub disjunct_count: usize,
    /// The leading `TripleConstraint`'s predicate from each disjunct (one per
    /// disjunct). The diagnostic uses this to name what's being split.
    pub disjunct_predicates: Vec<IriS>,
    /// The `OneOf` node, retained so `rejection_of` can walk its branches.
    pub suggestion_seed: SuggestionSeed,
}

/// The `OneOf` AST node retained from the construction walk.
///
/// `rejection_of` walks it to collect each branch's predicate IRIs, which is
/// the only thing that crosses into the format-neutral vocabulary. The name is
/// a leftover from when this crate rendered the suggestion itself and should be
/// read as "the node the split is derived from".
#[derive(Debug, Clone)]
pub struct SuggestionSeed {
    /// The `OneOf` node itself (cloned).
    pub one_of_node: TripleExpr,
}

// ---------------------------------------------------------------------------
// ShExDescriptor
// ---------------------------------------------------------------------------

/// Wraps a [`Schema`] with a pre-resolved per-shape constraint table.
///
/// Construct via [`ShExDescriptor::from_reader`] (JSON `ShEx` schema) or
/// [`ShExDescriptor::from_schema`] (already-parsed `Schema`).
#[derive(Debug)]
pub struct ShExDescriptor {
    schema: Schema,
    /// In the order the document declares them. A `HashMap` lived here until
    /// `crates/fossil-shex/examples/declaration_order.rs` measured what that
    /// cost: `values()` handed back six different orders in six parses, because
    /// Rust seeds its hasher per process. Anything downstream that iterates —
    /// [`Self::to_graph_schema`] builds `nodes`/`edges` from this — was
    /// non-deterministic across runs, and `type { A, B } = io.shex(...)` binds
    /// positionally, which needs this order to be the file's. rudof preserves it for both `ShExC` and `ShExJ`; we were
    /// the ones throwing it away on insert.
    shapes: Vec<ShapeBinding>,
    /// Resolved IRI → index into `shapes`. Lookup only; never iterated.
    index: HashMap<String, usize>,
    errors: Vec<ShExLoweringError>,
    /// The `ShExC` text this was parsed from, when it was parsed from one.
    ///
    /// The AST carries no offsets — `shex_ast`'s own `Span` is `nom_locate` and
    /// lives in its parse errors — so the only way a decoded shape can say
    /// WHERE it declares a predicate is to look in the text. [`crate::spans`]
    /// does that looking and says why it is safe.
    ///
    /// `None` for [`Self::from_reader`], which is the `ShExJ` path: a JSON
    /// document's offsets are offsets in JSON, and nobody reading a report
    /// about a shape is looking at that.
    source: Option<String>,
}

impl ShExDescriptor {
    /// Parse a `ShEx` schema from a JSON byte stream and build the resolved
    /// constraint table.
    ///
    /// Uses [`Schema::from_reader`] — no network access, so nothing here can
    /// drag a transitive `tokio`/`reqwest` into the WASM gate.
    pub fn from_reader<R: std::io::Read>(rdr: R) -> Result<Self, ShExLoweringError> {
        let schema = Schema::from_reader(rdr)
            .map_err(|e| ShExLoweringError::MalformedSchema(e.to_string()))?;
        Self::from_schema(schema)
    }

    /// Parse a `ShEx` schema in EITHER compact (`ShExC`) or JSON (`ShExJ`) syntax,
    /// auto-detected by the first non-whitespace byte (`{` ⇒ `ShExJ`). `ShExC` is
    /// the human-authored canonical surface syntax; `ShExJ` is the interchange
    /// form. Both lower to the same constraint table, so the rest of the pipeline
    /// (and [`Self::to_graph_schema`]) is syntax-agnostic.
    ///
    /// # Errors
    /// [`ShExLoweringError::MalformedSchema`] if neither parser accepts the input.
    pub fn from_shex_source(src: &str) -> Result<Self, ShExLoweringError> {
        if src.trim_start().starts_with('{') {
            return Self::from_reader(src.as_bytes());
        }
        let base = IriS::new_unchecked("http://fossil.invalid/schema");
        let schema = ShExParser::parse(src, None, &base)
            .map_err(|e| ShExLoweringError::MalformedSchema(e.to_string()))?;
        let mut descriptor = Self::from_schema(schema)?;
        // Kept for [`crate::spans`], and only on this path — see the field.
        descriptor.source = Some(src.to_string());
        Ok(descriptor)
    }

    /// Lower this `ShEx` schema into the format-neutral vocabulary — the
    /// decoded document the middle of the compiler reads instead of a schema
    /// language. No `shex_ast` or `rudof_iri` type survives the crossing.
    ///
    /// Shapes keep the document's declaration order (`type { A, B } = io.shex(…)`
    /// binds by position). Every lowering error
    /// becomes a [`Rejection`]; the shapes that DID lower are still there, which
    /// is what the `ShEx`-typed original did too.
    #[must_use]
    pub fn to_output_shapes(&self) -> OutputShapes {
        let prefixmap = self.schema.prefixmap().unwrap_or_default();
        let shapes = self
            .shapes()
            .map(|binding| OutputShape {
                iri: binding.iri.to_string(),
                properties: binding
                    .constraints
                    .iter()
                    .map(|c| PropertyConstraint {
                        predicate: c.predicate.to_string(),
                        datatype: datatype_of(c.value_expr.as_ref()),
                        targets: edge_targets(c.value_expr.as_ref()),
                        occurs: c.cardinality,
                        // The spellings are rudof's, via `qualify`, so nothing
                        // here reads a `PREFIX` declaration a second time —
                        // which is the one way looking in the text could give a
                        // wrong answer rather than no answer.
                        span: self.source.as_deref().and_then(|src| {
                            crate::spans::predicate_span(
                                src,
                                &prefixmap.qualify(&binding.iri),
                                &prefixmap.qualify(&c.predicate),
                            )
                        }),
                    })
                    .collect(),
            })
            .collect();
        let rejections = self
            .errors
            .iter()
            .map(|e| rejection_of(e, &prefixmap))
            .collect();
        OutputShapes::new(shapes, rejections)
    }

    /// Lower this `ShEx` schema into the canonical, format-neutral
    /// [`GraphSchema`] — the single output model the MIR/executor consume,
    /// shared with the (future) SHACL path.
    ///
    /// One path, not two: this is [`Self::to_output_shapes`] followed by
    /// [`OutputShapes::to_graph_schema`], so a `ShEx` document and a decoded
    /// document that says the same thing cannot lower differently. `renames`
    /// travels with it for the same reason — it governs the column label, and a
    /// forwarding method that dropped it would be a third answer.
    #[must_use]
    pub fn to_graph_schema(&self, renames: &Renames) -> GraphSchema {
        self.to_output_shapes().to_graph_schema(renames)
    }

    /// Build the resolved constraint table from an already-parsed schema.
    ///
    /// Lowering errors (`OneOf` encounters, cycles, unresolved refs) are
    /// pushed to [`Self::lowering_errors`]; the descriptor is still returned
    /// so consuming mappings can see the non-rejected parts of each shape.
    pub fn from_schema(schema: Schema) -> Result<Self, ShExLoweringError> {
        let prefixmap = schema.prefixmap().unwrap_or_default();
        let mut shapes: Vec<ShapeBinding> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut errors: Vec<ShExLoweringError> = Vec::new();

        // Declaration order, because the caller may bind by position. A repeated
        // IRI keeps its FIRST declaration and its first slot — the old `insert`
        // let the last one win silently, and either rule is arbitrary, but only
        // one of them leaves the order alone.
        for decl in schema.shapes().into_iter().flatten() {
            if let Some(binding) = lower_shape_decl(&decl, &prefixmap, &mut errors) {
                let iri = binding.iri.to_string();
                if index.contains_key(&iri) {
                    continue;
                }
                index.insert(iri, shapes.len());
                shapes.push(binding);
            }
        }

        Ok(Self {
            // `from_shex_source` fills this on the compact path; a schema that
            // arrived as an already-parsed AST has no text to point into.
            source: None,
            schema,
            shapes,
            index,
            errors,
        })
    }

    /// Borrow the underlying schema (for callers that want to walk the AST
    /// directly).
    #[must_use]
    pub const fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Look up a shape by its resolved IRI.
    #[must_use]
    pub fn lookup_shape(&self, iri: &IriS) -> Option<&ShapeBinding> {
        self.lookup_shape_str(&iri.to_string())
    }

    /// Look up a shape by its resolved IRI string — for callers that hold the
    /// shape IRI as a `&str` (e.g. the input descriptor) and don't want to
    /// construct an [`IriS`].
    #[must_use]
    pub fn lookup_shape_str(&self, iri: &str) -> Option<&ShapeBinding> {
        self.index.get(iri).map(|&i| &self.shapes[i])
    }

    /// Iterator over every resolved shape binding, **in declaration order**.
    /// Callers may rely on that: `type { A, B } = io.shex(…)` binds by position.
    pub fn shapes(&self) -> impl Iterator<Item = &ShapeBinding> {
        self.shapes.iter()
    }

    /// Errors discovered at construction time. The typecheck pass surfaces
    /// these as diagnostics keyed to the consuming mapping.
    #[must_use]
    pub fn lowering_errors(&self) -> &[ShExLoweringError] {
        &self.errors
    }
}

// ---------------------------------------------------------------------------
// Internal lowering helpers
// ---------------------------------------------------------------------------

/// Lower a single `ShapeDecl` to a [`ShapeBinding`], filling the per-shape
/// `TripleExpr::Ref` lookup table first and then walking the body once.
fn lower_shape_decl(
    decl: &ShapeDecl,
    prefixmap: &PrefixMap,
    errors: &mut Vec<ShExLoweringError>,
) -> Option<ShapeBinding> {
    let shape_iri = match &decl.id {
        ShapeExprLabel::IriRef { value } => resolve_iri_ref(value, prefixmap)?,
        ShapeExprLabel::BNode { .. } | ShapeExprLabel::Start => {
            // BNode / Start shape declarations are out of scope: only
            // IRI-identified shapes participate in backward checking.
            return None;
        }
    };

    // Only `ShapeExpr::Shape(_)` is supported. Other variants
    // (`ShapeOr` / `ShapeAnd` / `ShapeNot` / `External` / `NodeConstraint` /
    // `Ref`) are deferred.
    let ShapeExpr::Shape(shape) = &decl.shape_expr else {
        return None;
    };

    let label_table = build_label_table(shape);

    let mut visited: HashSet<String> = HashSet::new();
    let mut constraints: Vec<ResolvedConstraint> = Vec::new();

    if let Some(wrapper) = &shape.expression {
        walk_triple_expr(
            &wrapper.te,
            &shape_iri,
            prefixmap,
            &label_table,
            &mut visited,
            &mut constraints,
            errors,
        );
    }

    // `shape.closed` is read here and thrown away, deliberately: the field it
    // used to fill is gone from `fossil_graph_schema::Shape`. Nothing downstream
    // ever read it, and a bare property key is resolved against the declared
    // predicates either way — see the tombstone in `graph-schema/src/shapes.rs`.
    Some(ShapeBinding {
        iri: shape_iri,
        constraints,
    })
}

/// Build a `label -> TripleExpr` lookup for the body of a single shape.
///
/// Only inspects directly-nested `TripleExpr::EachOf` / `TripleExpr::OneOf` /
/// `TripleExpr::TripleConstraint` nodes carrying an explicit `id`.
fn build_label_table(shape: &Shape) -> HashMap<String, TripleExpr> {
    let mut table = HashMap::new();
    if let Some(wrapper) = &shape.expression {
        collect_labels(&wrapper.te, &mut table);
    }
    table
}

fn collect_labels(expr: &TripleExpr, table: &mut HashMap<String, TripleExpr>) {
    match expr {
        TripleExpr::EachOf {
            id, expressions, ..
        }
        | TripleExpr::OneOf {
            id, expressions, ..
        } => {
            if let Some(label) = id {
                table.insert(label_to_string(label), expr.clone());
            }
            for w in expressions {
                collect_labels(&w.te, table);
            }
        }
        TripleExpr::TripleConstraint { id, .. } => {
            if let Some(label) = id {
                table.insert(label_to_string(label), expr.clone());
            }
        }
        TripleExpr::Ref(_) => {}
    }
}

/// DFS-walk one `TripleExpr`, accumulating resolved constraints into `out`.
/// On `OneOf` push a rejection and stop descending. On `Ref` resolve via
/// `label_table`, recursing with cycle protection.
fn walk_triple_expr(
    expr: &TripleExpr,
    shape_iri: &IriS,
    prefixmap: &PrefixMap,
    label_table: &HashMap<String, TripleExpr>,
    visited: &mut HashSet<String>,
    out: &mut Vec<ResolvedConstraint>,
    errors: &mut Vec<ShExLoweringError>,
) {
    match expr {
        TripleExpr::TripleConstraint {
            predicate,
            value_expr,
            min,
            max,
            ..
        } => {
            let Some(pred_iri) = resolve_iri_ref(predicate, prefixmap) else {
                return;
            };
            out.push(ResolvedConstraint {
                predicate: pred_iri,
                value_expr: value_expr.as_ref().map(|b| (**b).clone()),
                cardinality: occurs_from_shex(*min, *max),
            });
        }
        TripleExpr::EachOf { expressions, .. } => {
            for w in expressions {
                walk_triple_expr(
                    &w.te,
                    shape_iri,
                    prefixmap,
                    label_table,
                    visited,
                    out,
                    errors,
                );
            }
        }
        TripleExpr::OneOf { expressions, .. } => {
            let disjunct_predicates = expressions
                .iter()
                .filter_map(|w| leading_predicate(&w.te, prefixmap))
                .collect::<Vec<_>>();
            errors.push(ShExLoweringError::OneOfRejection(OneOfRejection {
                shape_iri: shape_iri.clone(),
                disjunct_count: expressions.len(),
                disjunct_predicates,
                suggestion_seed: SuggestionSeed {
                    one_of_node: expr.clone(),
                },
            }));
            // Stop descending — a `OneOf` cannot be type-checked.
        }
        TripleExpr::Ref(label) => {
            let key = label_to_string(label);
            if !visited.insert(key.clone()) {
                errors.push(ShExLoweringError::CyclicShapeRef {
                    path: visited.iter().cloned().collect(),
                });
                return;
            }
            match label_table.get(&key) {
                Some(target) => {
                    walk_triple_expr(
                        target,
                        shape_iri,
                        prefixmap,
                        label_table,
                        visited,
                        out,
                        errors,
                    );
                }
                None => {
                    errors.push(ShExLoweringError::UnresolvedRef {
                        label: key.clone(),
                        in_shape: shape_iri.to_string(),
                    });
                }
            }
            visited.remove(&key);
        }
    }
}

/// Pull the first `TripleConstraint`'s predicate IRI from a disjunct's
/// `TripleExpr`, for reporting purposes.
fn leading_predicate(expr: &TripleExpr, prefixmap: &PrefixMap) -> Option<IriS> {
    match expr {
        TripleExpr::TripleConstraint { predicate, .. } => resolve_iri_ref(predicate, prefixmap),
        TripleExpr::EachOf { expressions, .. } | TripleExpr::OneOf { expressions, .. } => {
            expressions
                .first()
                .and_then(|w| leading_predicate(&w.te, prefixmap))
        }
        TripleExpr::Ref(_) => None,
    }
}

fn resolve_iri_ref(iri_ref: &IriRef, prefixmap: &PrefixMap) -> Option<IriS> {
    match iri_ref {
        IriRef::Iri(iri) => Some(iri.clone()),
        IriRef::Prefixed { prefix, local } => prefixmap
            .resolve_prefix_local(prefix, local)
            .ok()
            .or_else(|| {
                // Fallback: concat the prefix + local literally so callers
                // can still see the surface form. Real prefix-resolution
                // failure is rare in well-formed schemas.
                let s = format!("{prefix}{local}");
                Some(IriS::new_unchecked(s.as_str()))
            }),
    }
}

fn label_to_string(label: &TripleExprLabel) -> String {
    match label {
        TripleExprLabel::IriRef { value } => match value {
            IriRef::Iri(iri) => iri.to_string(),
            IriRef::Prefixed { prefix, local } => format!("{prefix}:{local}"),
        },
        TripleExprLabel::BNode { value } => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// `literal_string_with_formatting_args` flags the test fixture's Fossil IRI
// template syntax (`${ex:}user/${.id}`) as if it were an inline-format-args
// candidate. It's literal Fossil source — not a Rust format string.
#[allow(clippy::literal_string_with_formatting_args)]
mod tests {
    use super::*;

    /// The range survives the whole decode, and it is the range of the
    /// PREDICATE — not of the line, not of the shape.
    ///
    /// `crate::spans` proves the lookup over strings; this proves the wiring:
    /// that `from_shex_source` keeps the text, that `to_output_shapes` asks
    /// with rudof's own qualified spellings, and that what comes out the
    /// neutral end still points at the document. Sliced out of the source,
    /// because the numbers are the thing under test.
    #[test]
    fn a_compact_document_carries_where_it_declares_each_predicate() {
        const SRC: &str = "\
PREFIX shop: <https://shop.example/voc#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>

shop:Order {
  shop:total xsd:float
}
";
        let shapes = ShExDescriptor::from_shex_source(SRC)
            .expect("the document parses")
            .to_output_shapes();
        let first = shapes.shapes().next().expect("one shape");
        let property = &first.properties[0];
        assert_eq!(property.predicate, "https://shop.example/voc#total");
        assert_eq!(
            property.span.and_then(|s| s.slice(SRC)),
            Some("shop:total"),
            "the range is the predicate as the document spells it"
        );
    }

    /// And `ShExJ` carries none. The offsets would be offsets in JSON, which is
    /// not the document anybody is reading a report about — so `None`, which is
    /// the same answer SHACL gives and which every consumer already handles.
    #[test]
    fn a_json_document_carries_no_range() {
        let shapes = ShExDescriptor::from_shex_source(PERSON_NAME_SCHEMA)
            .expect("the document parses")
            .to_output_shapes();
        let first = shapes.shapes().next().expect("one shape");
        assert!(first.properties[0].span.is_none());
    }

    /// Minimal `ShEx` schema in JSON form — `ex:Person` with one
    /// `ex:name xsd:string` constraint.
    const PERSON_NAME_SCHEMA: &str = r#"{
      "@context": "http://www.w3.org/ns/shex.jsonld",
      "type": "Schema",
      "shapes": [
        {
          "type": "ShapeDecl",
          "id": "http://example.org/Person",
          "shapeExpr": {
            "type": "Shape",
            "expression": {
              "type": "TripleConstraint",
              "predicate": "http://example.org/name",
              "valueExpr": {
                "type": "NodeConstraint",
                "datatype": "http://www.w3.org/2001/XMLSchema#string"
              }
            }
          }
        }
      ]
    }"#;

    /// `ex:Contact` with a `OneOf` over (`ex:email` | `ex:phone`).
    const CONTACT_ONEOF_SCHEMA: &str = r#"{
      "@context": "http://www.w3.org/ns/shex.jsonld",
      "type": "Schema",
      "shapes": [
        {
          "type": "ShapeDecl",
          "id": "http://example.org/Contact",
          "shapeExpr": {
            "type": "Shape",
            "expression": {
              "type": "OneOf",
              "expressions": [
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/email",
                  "valueExpr": {
                    "type": "NodeConstraint",
                    "datatype": "http://www.w3.org/2001/XMLSchema#string"
                  }
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/phone",
                  "valueExpr": {
                    "type": "NodeConstraint",
                    "datatype": "http://www.w3.org/2001/XMLSchema#string"
                  }
                }
              ]
            }
          }
        }
      ]
    }"#;

    /// Build a schema where `ex:Person`'s body is an `EachOf` containing
    /// (a) a labelled inner `EachOf` `&PersonProps` declaring `ex:name`
    /// + `ex:age`, and (b) a `Ref` back to `PersonProps`.
    ///
    /// `shex_ast`'s JSON form for `Ref` is a bare-string IRI inside a
    /// `TripleExprWrapper` (per `serde_string_or_struct`), which is fiddly
    /// to spell as a JSON literal — we build the AST directly instead.
    fn ref_resolution_schema() -> Schema {
        let person_props_inner = TripleExpr::EachOf {
            id: Some(TripleExprLabel::IriRef {
                value: IriRef::Iri(IriS::new_unchecked("http://example.org/PersonProps")),
            }),
            expressions: vec![
                TripleExprWrapper {
                    te: TripleExpr::TripleConstraint {
                        id: None,
                        negated: None,
                        inverse: None,
                        predicate: IriRef::Iri(IriS::new_unchecked("http://example.org/name")),
                        value_expr: None,
                        min: None,
                        max: None,
                        sem_acts: None,
                        annotations: None,
                    },
                },
                TripleExprWrapper {
                    te: TripleExpr::TripleConstraint {
                        id: None,
                        negated: None,
                        inverse: None,
                        predicate: IriRef::Iri(IriS::new_unchecked("http://example.org/age")),
                        value_expr: None,
                        min: None,
                        max: None,
                        sem_acts: None,
                        annotations: None,
                    },
                },
            ],
            min: None,
            max: None,
            sem_acts: None,
            annotations: None,
        };
        let body = TripleExpr::EachOf {
            id: None,
            expressions: vec![
                TripleExprWrapper {
                    te: person_props_inner,
                },
                TripleExprWrapper {
                    te: TripleExpr::Ref(TripleExprLabel::IriRef {
                        value: IriRef::Iri(IriS::new_unchecked("http://example.org/PersonProps")),
                    }),
                },
            ],
            min: None,
            max: None,
            sem_acts: None,
            annotations: None,
        };
        let shape = Shape::new(None, None, Some(body));
        let mut schema = Schema::new(&IriS::new_unchecked("http://example.org/"));
        schema.add_shape(
            ShapeExprLabel::IriRef {
                value: IriRef::Iri(IriS::new_unchecked("http://example.org/Person")),
            },
            ShapeExpr::Shape(shape),
            false,
        );
        schema
    }

    /// Two named `TripleExpr`s in `ex:A`'s body that `Ref` each other,
    /// closing a cycle the DFS-visited check must catch.
    fn cyclic_schema() -> Schema {
        let inner_a = TripleExpr::EachOf {
            id: Some(TripleExprLabel::IriRef {
                value: IriRef::Iri(IriS::new_unchecked("http://example.org/tA")),
            }),
            expressions: vec![TripleExprWrapper {
                te: TripleExpr::Ref(TripleExprLabel::IriRef {
                    value: IriRef::Iri(IriS::new_unchecked("http://example.org/tB")),
                }),
            }],
            min: None,
            max: None,
            sem_acts: None,
            annotations: None,
        };
        let inner_b = TripleExpr::EachOf {
            id: Some(TripleExprLabel::IriRef {
                value: IriRef::Iri(IriS::new_unchecked("http://example.org/tB")),
            }),
            expressions: vec![TripleExprWrapper {
                te: TripleExpr::Ref(TripleExprLabel::IriRef {
                    value: IriRef::Iri(IriS::new_unchecked("http://example.org/tA")),
                }),
            }],
            min: None,
            max: None,
            sem_acts: None,
            annotations: None,
        };
        let body = TripleExpr::EachOf {
            id: None,
            expressions: vec![
                TripleExprWrapper { te: inner_a },
                TripleExprWrapper { te: inner_b },
                TripleExprWrapper {
                    te: TripleExpr::Ref(TripleExprLabel::IriRef {
                        value: IriRef::Iri(IriS::new_unchecked("http://example.org/tA")),
                    }),
                },
            ],
            min: None,
            max: None,
            sem_acts: None,
            annotations: None,
        };
        let shape = Shape::new(None, None, Some(body));
        let mut schema = Schema::new(&IriS::new_unchecked("http://example.org/"));
        schema.add_shape(
            ShapeExprLabel::IriRef {
                value: IriRef::Iri(IriS::new_unchecked("http://example.org/A")),
            },
            ShapeExpr::Shape(shape),
            false,
        );
        schema
    }

    #[test]
    fn parses_minimal_shex_schema() {
        let desc =
            ShExDescriptor::from_reader(PERSON_NAME_SCHEMA.as_bytes()).expect("schema parses");
        let binding = desc
            .lookup_shape(&IriS::new_unchecked("http://example.org/Person"))
            .expect("ex:Person resolved");
        assert_eq!(binding.constraints.len(), 1);
        assert_eq!(
            binding.constraints[0].predicate.to_string(),
            "http://example.org/name"
        );
        assert_eq!(binding.constraints[0].cardinality, Occurs::ONE);
        assert!(desc.lowering_errors().is_empty());
    }

    #[test]
    fn lookup_shape_returns_none_for_unknown_iri() {
        let desc =
            ShExDescriptor::from_reader(PERSON_NAME_SCHEMA.as_bytes()).expect("schema parses");
        assert!(
            desc.lookup_shape(&IriS::new_unchecked("http://example.org/Nope"))
                .is_none()
        );
    }

    /// Every `(min, max)` pair `ShEx` can hand over, and the `Occurs` it decodes
    /// to. The rows are the five cases the enum this replaces had names for,
    /// plus the three encodings that fell through to its `Range` arm — the pair
    /// is produced directly now, so there is no intermediate spelling to drift
    /// from. `(None, Some(-1))` is degenerate in both: `-1` only means
    /// "unbounded" beside an explicit `min`.
    #[test]
    fn occurs_decodes_every_shex_min_max_encoding() {
        let cases: &[(&str, Option<i32>, Option<i32>, Occurs)] = &[
            ("the default: exactly 1", None, None, Occurs::ONE),
            (
                "0..1",
                Some(0),
                Some(1),
                Occurs {
                    min: 0,
                    max: Some(1),
                },
            ),
            ("0..*", Some(0), Some(-1), Occurs { min: 0, max: None }),
            ("1..*", Some(1), Some(-1), Occurs { min: 1, max: None }),
            ("3..*", Some(3), Some(-1), Occurs { min: 3, max: None }),
            (
                "2..5",
                Some(2),
                Some(5),
                Occurs {
                    min: 2,
                    max: Some(5),
                },
            ),
            (
                "max only",
                None,
                Some(4),
                Occurs {
                    min: 1,
                    max: Some(4),
                },
            ),
            (
                "min only ⇒ exactly min",
                Some(2),
                None,
                Occurs {
                    min: 2,
                    max: Some(2),
                },
            ),
            (
                "a bare -1 max is not unbounded",
                None,
                Some(-1),
                Occurs {
                    min: 1,
                    max: Some(0),
                },
            ),
            (
                "negatives clamp to zero rather than panicking",
                Some(-7),
                Some(-3),
                Occurs {
                    min: 0,
                    max: Some(0),
                },
            ),
        ];
        for (name, min, max, want) in cases {
            assert_eq!(occurs_from_shex(*min, *max), *want, "{name}");
        }
    }

    #[test]
    fn shex_one_of_rejection() {
        let desc =
            ShExDescriptor::from_reader(CONTACT_ONEOF_SCHEMA.as_bytes()).expect("schema parses");
        let rejections: Vec<&OneOfRejection> = desc
            .lowering_errors()
            .iter()
            .filter_map(|e| match e {
                ShExLoweringError::OneOfRejection(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rejections.len(), 1);
        let r = rejections[0];
        assert_eq!(r.disjunct_count, 2);
        let predicates: Vec<String> = r
            .disjunct_predicates
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        assert!(predicates.iter().any(|p| p.contains("email")));
        assert!(predicates.iter().any(|p| p.contains("phone")));
    }

    #[test]
    fn shex_triple_expr_ref_resolution() {
        let schema = ref_resolution_schema();
        let desc = ShExDescriptor::from_schema(schema).expect("schema accepted");
        let binding = desc
            .lookup_shape(&IriS::new_unchecked("http://example.org/Person"))
            .expect("ex:Person resolved");

        let predicates: Vec<String> = binding
            .constraints
            .iter()
            .map(|c| c.predicate.to_string())
            .collect();
        // The PersonProps EachOf appears INLINE (label `id` declares it) AND
        // is referenced via the trailing `Ref`. Either way both `ex:name`
        // and `ex:age` are reachable.
        assert!(
            predicates.iter().any(|p| p == "http://example.org/name"),
            "ex:name missing: {predicates:?}"
        );
        assert!(
            predicates.iter().any(|p| p == "http://example.org/age"),
            "ex:age missing: {predicates:?}"
        );
    }

    #[test]
    fn shex_ref_cycle_detection() {
        let schema = cyclic_schema();
        let desc = ShExDescriptor::from_schema(schema).expect("schema accepted");
        let has_cycle = desc
            .lowering_errors()
            .iter()
            .any(|e| matches!(e, ShExLoweringError::CyclicShapeRef { .. }));
        assert!(
            has_cycle,
            "expected CyclicShapeRef, got {:?}",
            desc.lowering_errors()
        );
    }

    // -----------------------------------------------------------------------
    // ShEx → the neutral vocabulary
    // -----------------------------------------------------------------------

    /// Every case the lowering distinguishes, in one document: a literal with a
    /// datatype the lattice recognises, one with a datatype it does not, one
    /// with no `valueExpr` at all, a `nodeKind IRI` node, a single-target edge,
    /// a two-target `@<A> OR @<B>` edge — and every `(min, max)` encoding
    /// `ShEx` can spell.
    const ORDER_SCHEMA: &str = r#"{
      "@context": "http://www.w3.org/ns/shex.jsonld",
      "type": "Schema",
      "shapes": [
        {
          "type": "ShapeDecl",
          "id": "http://example.org/Order",
          "shapeExpr": {
            "type": "Shape",
            "closed": true,
            "expression": {
              "type": "EachOf",
              "expressions": [
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/total",
                  "valueExpr": {
                    "type": "NodeConstraint",
                    "datatype": "http://www.w3.org/2001/XMLSchema#integer"
                  }
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/code",
                  "valueExpr": {
                    "type": "NodeConstraint",
                    "datatype": "http://www.w3.org/2001/XMLSchema#hexBinary"
                  },
                  "min": 0,
                  "max": 1
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/note",
                  "min": 0,
                  "max": -1
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/homepage",
                  "valueExpr": { "type": "NodeConstraint", "nodeKind": "iri" },
                  "min": 1,
                  "max": -1
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/placedBy",
                  "valueExpr": "http://example.org/Person"
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/paidWith",
                  "valueExpr": {
                    "type": "ShapeOr",
                    "shapeExprs": [
                      "http://example.org/Card",
                      "http://example.org/Cash"
                    ]
                  },
                  "min": 2,
                  "max": 5
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/audited",
                  "min": 3
                },
                {
                  "type": "TripleConstraint",
                  "predicate": "http://example.org/reviewed",
                  "max": 4
                }
              ]
            }
          }
        },
        {
          "type": "ShapeDecl",
          "id": "http://example.org/Person",
          "shapeExpr": { "type": "Shape" }
        }
      ]
    }"#;

    fn order_shapes() -> OutputShapes {
        ShExDescriptor::from_reader(ORDER_SCHEMA.as_bytes())
            .expect("schema parses")
            .to_output_shapes()
    }

    /// The document's declaration order. This also asserted that `closed` was
    /// carried over per shape, and it was the ONLY reader of that field in the
    /// workspace — the checker never saw it. The field is gone; the ordering
    /// half is the half the positional `type { A, B }` binding relies on.
    #[test]
    fn output_shapes_keep_declaration_order() {
        let doc = order_shapes();
        let order: Vec<&str> = doc.shapes().map(|s| s.iri.as_str()).collect();
        assert_eq!(
            order,
            ["http://example.org/Order", "http://example.org/Person"],
            "declaration order, not hash order"
        );
        assert!(doc.rejections().is_empty());
    }

    /// `datatype` says what the DOCUMENT narrowed the value to. `None` is not
    /// "string" — it is "the document did not say", and an XSD datatype outside
    /// the lattice is one of the ways a document does not say.
    #[test]
    fn a_datatype_is_some_only_when_the_document_narrowed_it() {
        let doc = order_shapes();
        let order = doc.lookup("http://example.org/Order").expect("Order");
        let by_predicate = |local: &str| {
            order
                .properties
                .iter()
                .find(|p| local_name(&p.predicate) == local)
                .unwrap_or_else(|| panic!("no `{local}` property"))
        };

        assert_eq!(
            by_predicate("total").datatype,
            Some(Primitive::Integer),
            "a datatype the lattice recognises"
        );
        assert_eq!(
            by_predicate("code").datatype,
            None,
            "xsd:hexBinary is a real XSD datatype OUTSIDE the lattice, and \
             `None` is how that is said"
        );
        assert_eq!(by_predicate("note").datatype, None, "no valueExpr at all");
        assert_eq!(
            by_predicate("homepage").datatype,
            Some(Primitive::AnyUri),
            "`nodeKind IRI` is an opaque IRI column — narrowed, but not an edge"
        );
        assert!(
            by_predicate("homepage").targets.is_empty(),
            "an opaque IRI is a column, not an edge"
        );
    }

    /// `targets` is the edge destination set: empty for a literal, one for a
    /// shape `Ref`, and every ref of a `ShapeOr` in the order it is written.
    #[test]
    fn targets_carry_the_edge_destinations_in_order() {
        let doc = order_shapes();
        let order = doc.lookup("http://example.org/Order").expect("Order");
        let by_predicate = |local: &str| {
            order
                .properties
                .iter()
                .find(|p| local_name(&p.predicate) == local)
                .unwrap_or_else(|| panic!("no `{local}` property"))
        };

        assert!(by_predicate("total").targets.is_empty());
        assert_eq!(
            by_predicate("placedBy").targets,
            ["http://example.org/Person"]
        );
        assert_eq!(
            by_predicate("paidWith").targets,
            ["http://example.org/Card", "http://example.org/Cash"],
            "`@<A> OR @<B>` keeps both destinations, in the written order"
        );
    }

    /// Every `(min, max)` arm, reached through a real parse rather than through
    /// `occurs_from_shex` directly — the encodings the `ShExJ` form can carry.
    #[test]
    fn occurs_survives_the_parse_for_every_encoding() {
        let doc = order_shapes();
        let order = doc.lookup("http://example.org/Order").expect("Order");
        let occurs = |local: &str| {
            order
                .properties
                .iter()
                .find(|p| local_name(&p.predicate) == local)
                .unwrap_or_else(|| panic!("no `{local}` property"))
                .occurs
        };

        assert_eq!(occurs("total"), Occurs::ONE, "omitted ⇒ exactly 1");
        assert_eq!(
            occurs("code"),
            Occurs {
                min: 0,
                max: Some(1)
            },
            "0..1"
        );
        assert_eq!(occurs("note"), Occurs { min: 0, max: None }, "0..*");
        assert_eq!(occurs("homepage"), Occurs { min: 1, max: None }, "1..*");
        assert_eq!(
            occurs("paidWith"),
            Occurs {
                min: 2,
                max: Some(5)
            },
            "2..5"
        );
        assert_eq!(
            occurs("audited"),
            Occurs {
                min: 3,
                max: Some(3)
            },
            "min alone ⇒ exactly min"
        );
        assert_eq!(
            occurs("reviewed"),
            Occurs {
                min: 1,
                max: Some(4)
            },
            "max alone ⇒ 1..max"
        );
    }

    /// A `OneOf` becomes a [`Rejection::Disjunction`] carrying each branch's
    /// predicate IRIs **in order** — enough to name the split with no `ShEx`
    /// type in hand. A branch that is an `EachOf` contributes all of its
    /// predicates; a nested `OneOf` contributes none and is not a second
    /// rejection (the outer walk already stopped).
    #[test]
    fn a_one_of_becomes_a_disjunction_carrying_each_branchs_predicates() {
        const SCHEMA: &str = r#"{
          "@context": "http://www.w3.org/ns/shex.jsonld",
          "type": "Schema",
          "shapes": [
            {
              "type": "ShapeDecl",
              "id": "http://example.org/Contact",
              "shapeExpr": {
                "type": "Shape",
                "expression": {
                  "type": "OneOf",
                  "expressions": [
                    {
                      "type": "EachOf",
                      "expressions": [
                        { "type": "TripleConstraint", "predicate": "http://example.org/email" },
                        { "type": "TripleConstraint", "predicate": "http://example.org/emailVerified" }
                      ]
                    },
                    { "type": "TripleConstraint", "predicate": "http://example.org/phone" },
                    {
                      "type": "OneOf",
                      "expressions": [
                        { "type": "TripleConstraint", "predicate": "http://example.org/fax" }
                      ]
                    }
                  ]
                }
              }
            }
          ]
        }"#;

        let doc = ShExDescriptor::from_reader(SCHEMA.as_bytes())
            .expect("schema parses")
            .to_output_shapes();

        assert_eq!(
            doc.rejections(),
            [Rejection::Disjunction {
                shape_iri: "http://example.org/Contact".into(),
                disjuncts: vec![
                    vec![
                        "http://example.org/email".to_string(),
                        "http://example.org/emailVerified".to_string(),
                    ],
                    vec!["http://example.org/phone".to_string()],
                    vec![],
                ],
            }],
            "three branches in order; the nested OneOf contributes an empty one \
             and is not a second rejection"
        );

        // The rejected `OneOf` is not silently half-lowered into properties.
        assert_eq!(
            doc.lookup("http://example.org/Contact")
                .expect("the shape is still there")
                .properties
                .len(),
            0
        );
    }

    /// The document that did not parse produces the rejection and nothing else
    /// — and it arrives as an `Err`, which is what the decoder row turns into
    /// `OutputShapes::rejected`.
    #[test]
    fn a_document_that_does_not_parse_is_a_malformed_schema() {
        let err = ShExDescriptor::from_shex_source("{ not json").expect_err("must not parse");
        assert!(
            matches!(err, ShExLoweringError::MalformedSchema(_)),
            "got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // to_graph_schema goes through the neutral vocabulary, unchanged
    // -----------------------------------------------------------------------

    /// The output model, constructed **by hand** rather than snapshotted
    /// against ourselves. Every distinction the old direct lowering made is
    /// here: an un-narrowed value is a `String` column, a `nodeKind IRI` is an
    /// `AnyUri` column and not an edge, a disjunction emits one edge per
    /// destination sharing the predicate, and the single/multi collapse follows
    /// `Occurs::is_single_valued`.
    #[test]
    fn to_graph_schema_through_the_neutral_vocabulary_is_unchanged() {
        use fossil_graph_schema::{Cardinality, EdgeType, NodeType, Property};

        let g = ShExDescriptor::from_reader(ORDER_SCHEMA.as_bytes())
            .expect("schema parses")
            .to_graph_schema(&Renames::default());

        let prop = |name: &str, datatype: Primitive, iri: &str, cardinality| Property {
            name: name.into(),
            datatype,
            iri: Some(iri.into()),
            cardinality,
        };
        let edge = |label: &str, iri: &str, destination: &str, cardinality| EdgeType {
            label: label.into(),
            iri: Some(iri.into()),
            source: "Order".into(),
            destination: destination.into(),
            cardinality,
        };

        assert_eq!(
            g.nodes,
            vec![
                NodeType {
                    label: "Order".into(),
                    iri: Some("http://example.org/Order".into()),
                    properties: vec![
                        prop(
                            "total",
                            Primitive::Integer,
                            "http://example.org/total",
                            Cardinality::Single
                        ),
                        prop(
                            "code",
                            Primitive::String,
                            "http://example.org/code",
                            Cardinality::Single
                        ),
                        prop(
                            "note",
                            Primitive::String,
                            "http://example.org/note",
                            Cardinality::Multi
                        ),
                        prop(
                            "homepage",
                            Primitive::AnyUri,
                            "http://example.org/homepage",
                            Cardinality::Multi
                        ),
                        prop(
                            "audited",
                            Primitive::String,
                            "http://example.org/audited",
                            Cardinality::Multi
                        ),
                        prop(
                            "reviewed",
                            Primitive::String,
                            "http://example.org/reviewed",
                            Cardinality::Multi
                        ),
                    ],
                },
                NodeType {
                    label: "Person".into(),
                    iri: Some("http://example.org/Person".into()),
                    properties: vec![],
                },
            ]
        );

        assert_eq!(
            g.edges,
            vec![
                edge(
                    "placedBy",
                    "http://example.org/placedBy",
                    "Person",
                    Cardinality::Single
                ),
                edge(
                    "paidWith",
                    "http://example.org/paidWith",
                    "Card",
                    Cardinality::Multi
                ),
                edge(
                    "paidWith",
                    "http://example.org/paidWith",
                    "Cash",
                    Cardinality::Multi
                ),
            ]
        );
    }
}
