//! `ShExDescriptor` — wraps a [`shex_ast::Schema`] for Fossil's static
//! target-shape checking (backward shape checking, CORE-06).
//!
//! ## Design summary
//!
//! Fossil's bidirectional checker (Phase 3 plan 03-05) wants to ask "given
//! target shape `ex:Person`, what is the expected type + cardinality of
//! predicate `p`?". That's a per-shape constraint table. `ShEx` 2.1 lets
//! shapes share triple-expressions via `TripleExpr::Ref(TripleExprLabel)`
//! (forward reference to a named `EachOf`/`OneOf`/`TripleConstraint`
//! declared elsewhere in the same schema).
//!
//! We pre-resolve the AST at construction time (`RESEARCH.md` §Pitfall 5):
//! walking a shape body without resolving refs produces "unknown property"
//! false positives. Cycle detection during walk (DFS-visited) — only acyclic
//! shape graphs are supported per `type-system.md` §11.
//!
//! ## `OneOf` rejection at shape-lowering time (NOT per-mapping-check)
//!
//! SC#4 requires one deterministic compile error per `OneOf` encounter + a
//! generated Fossil source split-into-N-mappings code suggestion. Doing this
//! at per-mapping-check time would duplicate the diagnostic across every
//! consuming mapping. Instead we collect `ShExLoweringError::OneOfRejection`
//! during the construction walk; plan 03-05's typecheck emits the diagnostic
//! once, attaching `Diagnostic.suggestion_source` populated from
//! [`generate_split_suggestion`].
//!
//! ## WASM safety (Pitfall 1)
//!
//! We use ONLY `Schema::from_reader` (byte-stream input) — never
//! `Schema::from_iri` which would pull `reqwest`/`tokio` into the WASM build
//! path. Verified by the Phase 0 spike (`decisions/rudof-wasm.md`) and the
//! plan 03-01 re-verification.

// `result_large_err`: `ShExLoweringError` carries a `TripleExpr` via
// `OneOfRejection::suggestion_seed::one_of_node`. The two `pub` constructors
// return `Result<Self, ShExLoweringError>` only at the top-level
// `MalformedSchema(String)` failure path — the large variants only flow
// through `lowering_errors()`, never via `Result`. Boxing the entire enum
// would hurt every other consumer.
#![allow(clippy::result_large_err)]

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use fossil_graph_schema::{
    Cardinality as GsCardinality, EdgeType, GraphSchema, NodeType, Primitive, Property,
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
/// bidirectional checker (plan 03-05).
#[derive(Debug, Clone)]
pub struct ShapeBinding {
    /// The shape's declared IRI (after prefix resolution).
    pub iri: IriS,
    /// Whether the shape is `closed`.
    pub closed: bool,
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
    /// for now — plan 03-05 narrows this into a `Ty<'db>` against the
    /// `Primitive` lattice.
    pub value_expr: Option<ShapeExpr>,
    /// Cardinality decoded from `ShEx`'s `(min, max)` integer encoding.
    pub cardinality: Cardinality,
}

/// Decoded cardinality of a [`ResolvedConstraint`].
///
/// Matches the shape from `RESEARCH.md` §"Bidirectional Checker Shape"
/// Example 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    Exact(u32),
    ZeroOrOne,
    OneOrMore,
    ZeroOrMore,
    Range { min: u32, max: Option<u32> },
}

impl Cardinality {
    /// Convert from `ShEx`'s `(min, max)` `Option<i32>` encoding.
    ///
    /// Per [the ShEx 2.1 spec](https://shex.io/shex-semantics/), `max = -1`
    /// means "unbounded" in the JSON form. `(None, None)` means "exactly 1"
    /// (the default).
    #[must_use]
    pub fn from_shex(min: Option<i32>, max: Option<i32>) -> Self {
        // Negative `lo`/`hi` values shouldn't occur in well-formed schemas
        // (ShEx 2.1 only uses `-1` to encode "unbounded" in `max`), but if
        // they do we clamp to zero rather than panicking.
        let to_u32 = |v: i32| u32::try_from(v.max(0)).unwrap_or(0);
        match (min, max) {
            (None, None) => Self::Exact(1),
            (Some(0), Some(1)) => Self::ZeroOrOne,
            (Some(0), Some(-1)) => Self::ZeroOrMore,
            (Some(1), Some(-1)) => Self::OneOrMore,
            (Some(lo), Some(-1)) => Self::Range {
                min: to_u32(lo),
                max: None,
            },
            (Some(lo), Some(hi)) => Self::Range {
                min: to_u32(lo),
                max: Some(to_u32(hi)),
            },
            (None, Some(hi)) => Self::Range {
                min: 1,
                max: Some(to_u32(hi)),
            },
            (Some(lo), None) => Self::Range {
                min: to_u32(lo),
                max: Some(to_u32(lo)),
            },
        }
    }

    /// `true` iff at most one value is allowed — `Exact(_)` / `ZeroOrOne`
    /// (or a `Range` with `max <= 1`). Single-valued constraints collapse
    /// duplicate subjects; `OneOrMore` / `ZeroOrMore` keep every value (and, for
    /// the RDF provider, become a `LIST` column). The single source of this
    /// truth, shared by the input pivot (`fossil-provider-rdf`) and the output
    /// decomposition (`fossil-sinks`).
    #[must_use]
    pub const fn is_single_valued(self) -> bool {
        match self {
            Self::Exact(_) | Self::ZeroOrOne => true,
            Self::OneOrMore | Self::ZeroOrMore => false,
            Self::Range { max, .. } => matches!(max, Some(m) if m <= 1),
        }
    }
}

/// The Fossil-relevant narrowing of a [`ResolvedConstraint`]'s `ShEx` `valueExpr`.
///
/// `ShEx`'s `valueExpr` is a full `ShapeExpr` lattice; Fossil only needs to know,
/// per property, whether the value is a typed literal (→ a scalar column of a
/// known primitive), an IRI / object reference (→ an edge / IRI-valued column),
/// or something it cannot narrow yet. This is the single decode of that
/// question, shared by the INPUT descriptor (deriving source column types,
/// compile-time) and the OUTPUT bidirectional checker (plan 03-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintValue {
    /// A literal constrained to this datatype IRI (e.g.
    /// `http://www.w3.org/2001/XMLSchema#integer`). The consumer maps the IRI
    /// to its own type lattice.
    Datatype(String),
    /// An IRI-valued node (`nodeKind IRI`) or a reference to another shape — an
    /// object property. The value is the referenced subject's IRI. (The OUTPUT
    /// decomposition's `classify_object` is what turns a shape-ref into a typed
    /// `GraphAr` edge; this INPUT narrowing only needs "is it an IRI".)
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

    /// The destination shape's IRI when this constraint is an inter-shape edge
    /// (`value_expr` is a shape `Ref`) — the property decomposes to an edge
    /// `S --predicate--> <returned IRI>`. `None` for a literal or opaque-IRI
    /// property (those stay vertex columns).
    ///
    /// The single source of the edge target, shared by the output decomposition
    /// (`fossil-sinks`) and the property-graph MIR lowering (`fossil-mir`) —
    /// both must agree on which constraints become edges. Mirrors the `Ref` arm
    /// of `fossil-sinks`'s `classify_object` (an inline `Shape` is not an edge;
    /// `decompose`'s `Edge` kind is `Ref`-only).
    #[must_use]
    pub fn edge_target(&self) -> Option<String> {
        match &self.value_expr {
            Some(ShapeExpr::Ref(label)) => Some(shape_label_iri(label)),
            _ => None,
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

/// The local name of an IRI — the substring after the last `#` or `/`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// The destination shape IRIs of an edge constraint — empty for a literal/opaque
/// property. A single shape `Ref` yields one; a value disjunction `@<A> OR @<B>`
/// (`ShapeOr` of refs) yields all of them, so the canonical model emits one edge
/// type per destination. `ShapeAnd`/`ShapeNot`/`NodeConstraint`/inline `Shape`
/// are not inter-shape edges.
fn edge_targets(value_expr: &Option<ShapeExpr>) -> Vec<String> {
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

/// The canonical datatype of a non-edge constraint: a typed literal maps through
/// the XSD lattice (unknown datatypes fall back to `String`); a `nodeKind IRI`
/// node is an opaque IRI-valued property (`AnyUri`); anything else defaults to
/// `String` (the permissive walking-skeleton column).
fn datatype_of(value_expr: &Option<ShapeExpr>) -> Primitive {
    match value_expr {
        Some(ShapeExpr::NodeConstraint(nc)) => nc.datatype().map_or_else(
            || {
                if matches!(nc.node_kind(), Some(NodeKind::Iri)) {
                    Primitive::AnyUri
                } else {
                    Primitive::String
                }
            },
            |dt| Primitive::from_xsd_iri(&iri_ref_to_string(&dt)).unwrap_or(Primitive::String),
        ),
        _ => Primitive::String,
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
/// tables. Surfaced via [`ShExDescriptor::lowering_errors`]; plan 03-05's
/// typecheck pass emits matching `Diagnostic`s.
#[derive(Debug, Clone)]
pub enum ShExLoweringError {
    /// SC#4 — caller emits a `Diagnostic` carrying the generated split
    /// suggestion in `suggestion_source`.
    OneOfRejection(OneOfRejection),
    /// `type-system.md` §11 — only acyclic shape graphs are supported.
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
    /// Re-emit data for the suggestion generator.
    pub suggestion_seed: SuggestionSeed,
}

/// Input to [`generate_split_suggestion`] retained from the original walk.
#[derive(Debug, Clone)]
pub struct SuggestionSeed {
    /// The `OneOf` node itself (cloned). Plan 03-05's emitter passes it back
    /// into [`generate_split_suggestion`] along with the consuming mapping's
    /// header values.
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
    /// non-deterministic across runs, and ADR-0057's tenth amendment binds
    /// `type { A, B } = io.shex(...)` positionally, which needs this order to
    /// be the file's. rudof preserves it for both `ShExC` and `ShExJ`; we were
    /// the ones throwing it away on insert.
    shapes: Vec<ShapeBinding>,
    /// Resolved IRI → index into `shapes`. Lookup only; never iterated.
    index: HashMap<String, usize>,
    errors: Vec<ShExLoweringError>,
}

impl ShExDescriptor {
    /// Parse a `ShEx` schema from a JSON byte stream and build the resolved
    /// constraint table.
    ///
    /// Uses [`Schema::from_reader`] — no network access (Pitfall 1).
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
        Self::from_schema(schema)
    }

    /// Lower this `ShEx` schema into the canonical, format-neutral
    /// [`GraphSchema`] — the single output model the MIR/executor consume,
    /// shared with the (future) SHACL path. Each shape becomes a [`NodeType`]
    /// (keyed by its `rdf:type` IRI); each triple constraint becomes either an
    /// [`EdgeType`] (value is a shape ref) or a literal/IRI [`Property`]. A value
    /// disjunction (`@<A> OR @<B>`) emits **one edge per destination**, sharing
    /// the predicate label — the reference RDF→property-graph model.
    #[must_use]
    pub fn to_graph_schema(&self) -> GraphSchema {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for binding in self.shapes() {
            let node_iri = binding.iri.to_string();
            let label = local_name(&node_iri).to_string();
            let mut properties = Vec::new();
            for c in &binding.constraints {
                let pred_iri = c.predicate.to_string();
                let name = local_name(&pred_iri).to_string();
                let cardinality = if c.cardinality.is_single_valued() {
                    GsCardinality::Single
                } else {
                    GsCardinality::Multi
                };
                let targets = edge_targets(&c.value_expr);
                if targets.is_empty() {
                    properties.push(Property {
                        name,
                        datatype: datatype_of(&c.value_expr),
                        iri: Some(pred_iri),
                        cardinality,
                    });
                } else {
                    for t in targets {
                        edges.push(EdgeType {
                            label: name.clone(),
                            iri: Some(pred_iri.clone()),
                            source: label.clone(),
                            destination: local_name(&t).to_string(),
                            cardinality,
                        });
                    }
                }
            }
            nodes.push(NodeType {
                label,
                iri: Some(node_iri),
                properties,
            });
        }
        GraphSchema { nodes, edges }
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
    /// Callers may rely on that: ADR-0057's tenth amendment binds by position.
    pub fn shapes(&self) -> impl Iterator<Item = &ShapeBinding> {
        self.shapes.iter()
    }

    /// Errors discovered at construction time. Plan 03-05 surfaces these as
    /// diagnostics keyed to the consuming mapping.
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
            // BNode / Start shape declarations are out of scope per Phase 3
            // v0.1 (only IRI-identified shapes participate in backward
            // checking).
            return None;
        }
    };

    // Phase 3 v0.1 supports `ShapeExpr::Shape(_)` only. Other variants
    // (`ShapeOr` / `ShapeAnd` / `ShapeNot` / `External` / `NodeConstraint` /
    // `Ref`) are deferred per `RESEARCH.md` §"Deferred Ideas".
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

    Some(ShapeBinding {
        iri: shape_iri,
        closed: shape.closed.unwrap_or(false),
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
                cardinality: Cardinality::from_shex(*min, *max),
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
            // Stop descending — Phase 3 v0.1 cannot type-check a OneOf.
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
// Split-into-N-mappings suggestion generator (SC#4)
// ---------------------------------------------------------------------------

/// Generate a Fossil source snippet that splits a `OneOf` into N separate
/// mappings — one per disjunct.
///
/// The output is a starting template that compiles in Fossil. Inferred CSV
/// field names follow the predicate's local part. Plan 03-05's diagnostic
/// emitter may prepend a hint like "this is a starting template — adjust
/// field names to match your CSVW columns".
///
/// # Panics
///
/// Panics if `one_of` is not the `TripleExpr::OneOf` variant. Callers always
/// have this guaranteed by the [`OneOfRejection::suggestion_seed`] path.
#[must_use]
pub fn generate_split_suggestion(
    base_mapping_name: &str,
    base_iri_template: &str,
    base_from_clause: &str,
    base_shape_iri: &str,
    one_of: &TripleExpr,
) -> String {
    let TripleExpr::OneOf { expressions, .. } = one_of else {
        panic!("generate_split_suggestion: expected TripleExpr::OneOf");
    };

    let mut out = String::new();
    for (i, disjunct) in expressions.iter().enumerate() {
        let idx = i + 1;
        // `write!` into a `String` is infallible.
        let _ = write!(
            out,
            "{base_mapping_name}{idx} : {base_shape_iri} from {base_from_clause}\n    iri = {base_iri_template}\n",
        );
        emit_disjunct_properties(&disjunct.te, &mut out);
        out.push('\n');
    }
    out
}

fn emit_disjunct_properties(expr: &TripleExpr, out: &mut String) {
    match expr {
        TripleExpr::TripleConstraint { predicate, .. } => {
            let (pred_text, field_name) = predicate_render(predicate);
            let _ = writeln!(out, "    {pred_text} = .{field_name}");
        }
        TripleExpr::EachOf { expressions, .. } => {
            for w in expressions {
                emit_disjunct_properties(&w.te, out);
            }
        }
        // Nested `OneOf` inside a `OneOf` disjunct collapses to a TODO line —
        // user must hand-split further. Phase 3 v0.1 rejects this case anyway
        // at the outer walk.
        TripleExpr::OneOf { .. } => {
            out.push_str("    # TODO: nested OneOf — split further\n");
        }
        TripleExpr::Ref(label) => {
            let _ = writeln!(out, "    # TODO: resolve ref {}", label_to_string(label));
        }
    }
}

/// Pretty-print a predicate `IriRef` for the suggestion text + infer a
/// reasonable CSV-column field name from its local part.
fn predicate_render(iri_ref: &IriRef) -> (String, String) {
    match iri_ref {
        IriRef::Prefixed { prefix, local } => (format!("{prefix}:{local}"), local.clone()),
        IriRef::Iri(iri) => {
            let s = iri.to_string();
            // Take the trailing path segment as the inferred field name.
            let inferred = s
                .rsplit_once(['/', '#', ':'])
                .map_or(s.as_str(), |(_, tail)| tail)
                .to_string();
            let field = if inferred.is_empty() {
                "field".to_string()
            } else {
                inferred
            };
            (format!("<{s}>"), field)
        }
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
        assert_eq!(binding.constraints[0].cardinality, Cardinality::Exact(1));
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

    #[test]
    fn cardinality_from_shex_min_none_max_none() {
        assert_eq!(Cardinality::from_shex(None, None), Cardinality::Exact(1));
    }

    #[test]
    fn cardinality_from_shex_zero_one() {
        assert_eq!(
            Cardinality::from_shex(Some(0), Some(1)),
            Cardinality::ZeroOrOne
        );
    }

    #[test]
    fn cardinality_from_shex_zero_unbounded() {
        assert_eq!(
            Cardinality::from_shex(Some(0), Some(-1)),
            Cardinality::ZeroOrMore
        );
    }

    #[test]
    fn cardinality_from_shex_one_unbounded() {
        assert_eq!(
            Cardinality::from_shex(Some(1), Some(-1)),
            Cardinality::OneOrMore
        );
    }

    #[test]
    fn cardinality_from_shex_range() {
        assert_eq!(
            Cardinality::from_shex(Some(2), Some(5)),
            Cardinality::Range {
                min: 2,
                max: Some(5)
            }
        );
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

        // Suggestion-seed round-trip: regenerate the split text and check
        // both predicates appear in two mapping headers.
        let suggestion = generate_split_suggestion(
            "UserContact",
            "`${ex:}user/${.id}`",
            "users",
            "ex:Person",
            &r.suggestion_seed.one_of_node,
        );
        assert!(
            suggestion.contains("UserContact1"),
            "first mapping not emitted: {suggestion}"
        );
        assert!(
            suggestion.contains("UserContact2"),
            "second mapping not emitted: {suggestion}"
        );
        assert!(
            suggestion.contains("email"),
            "email predicate missing: {suggestion}"
        );
        assert!(
            suggestion.contains("phone"),
            "phone predicate missing: {suggestion}"
        );
    }

    #[test]
    fn shex_one_of_rejection_suggestion_snapshot() {
        let desc =
            ShExDescriptor::from_reader(CONTACT_ONEOF_SCHEMA.as_bytes()).expect("schema parses");
        let r = desc
            .lowering_errors()
            .iter()
            .find_map(|e| match e {
                ShExLoweringError::OneOfRejection(r) => Some(r),
                _ => None,
            })
            .expect("OneOf rejection present");

        let suggestion = generate_split_suggestion(
            "UserContact",
            "`${ex:}user/${.id}`",
            "users",
            "ex:Person",
            &r.suggestion_seed.one_of_node,
        );

        // Verbatim snapshot — checked in here, NOT via insta, so the asset
        // travels with the test file (per plan 03-03 §output requirement
        // that the SUMMARY can paste it).
        let expected = "\
UserContact1 : ex:Person from users\n    \
iri = `${ex:}user/${.id}`\n    \
<http://example.org/email> = .email\n\n\
UserContact2 : ex:Person from users\n    \
iri = `${ex:}user/${.id}`\n    \
<http://example.org/phone> = .phone\n\n";
        assert_eq!(
            suggestion, expected,
            "split suggestion did not match snapshot"
        );
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
}
