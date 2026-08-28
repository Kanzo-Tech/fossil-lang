//! Typed operator algebra — the complete typed operator set.
//!
//! `Op<'db>` carries the eight relational operators of Min Oo &
//! Hartig's construction algebra (arXiv 2503.10385; ESWC 2025) — `Source`,
//! `Project`, `Extend`, `Rename`, `Filter`, `Join`, `Union`, `Distinct` — with
//! their `GroupBy` and `Aggregate` as ONE operator, for the reason written on
//! [`Op::GroupBy`] —
//! plus the typed-emission refinements this compiler adds in place of their
//! combined serializer/target step (`EmitVertex`, `EmitEdge`, `Sink`) and an
//! [`Op::Empty`] node, the empty relation that is the identity of `Union` and
//! the target a statically-false filter rewrites to. [`Expr`] carries [`Ty`]
//! on the synthesised nodes, which is the whole of what "typed" adds: the
//! untyped fragment is operationally their algebra.
//!
//! `/docs/design/algebra` is the page that argues it. **Completeness of the
//! IR is not surface coverage**: every operator is DEFINED here, and the ones
//! a program can reach are the ones a verb spells. `fossil_hir::stdlib::PlanOp`
//! is the list, and `fossil_hir::lower::lowered_verbs` derives which of them
//! have arrived; the rest are exercised by direct `MirGraph` construction.
//!
//! # Design notes
//!
//! - **Enum dispatch ONLY** (CLAUDE.md hard rule). `Op` / `Expr` use enum
//!   variants, never `Box<dyn Trait>`. Recursion *inside* an `Expr` variant via
//!   `Box<Expr>` is data, not a trait object — `salsa::Update` lifts through
//!   `Box<T>` transparently when `T: Update`, so no interned-newtype escape
//!   hatch is needed.
//! - `Op<'db>` / `Expr<'db>` carry `'db` because they reference [`Ty<'db>`]
//!   (interned handles). They are NOT themselves interned — they live inside
//!   the tracked [`crate::graph::MirGraph`] — so carrying `Ty` in `Hash`/`Eq`
//!   is fine and makes the "erase types ≡ untyped algebra" property testable.
//! - `input` / `left` / `right` are `usize` indices into
//!   [`crate::graph::MirGraph::ops`] in topological order. Two-input ops carry
//!   two indices.

use fossil_hir::BinOp;
use fossil_hir::Ty;
use fossil_hir::UnOp;
use fossil_hir::stdlib::AggFn;
use smol_str::SmolStr;

/// One node of the MIR DAG. The complete typed operator algebra: the nine
/// relational operators, the two typed-emission refinements plus [`Op::Sink`],
/// and [`Op::Empty`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Op<'db> {
    /// `SourceOp(uri, format, row_type)` — origin of all row data.
    ///
    /// `row_type` is the `Record` row type derived from the host-registered
    /// descriptor, or the typed source row (`TypeckOutput.source_row`).
    Source {
        uri: SmolStr,
        format: SourceFormat,
        row_type: Ty<'db>,
        /// The source BINDING name (`projects` in `projects := io.rdf(...)`).
        /// The relation/view name is derived from this, NOT the URI stem, so two
        /// bindings reading the same file (two destructuring members of one
        /// `io.rdf`) get distinct relations instead of colliding on the stem.
        binding: SmolStr,
    },

    /// `ProjectOp(input, cols)` — restrict the row to the selected columns.
    ///
    /// Each column names the relation it belongs to, because after a
    /// [`Op::Join`] a bare name does not identify one: two sides may both carry
    /// `id`, and which one `select(id)` meant was decided by the order the two
    /// were joined in. See [`ProjectedColumn`].
    Project {
        input: usize,
        cols: Vec<ProjectedColumn>,
    },

    /// `ExtendOp(input, field, expr)` — add a computed field whose type is the
    /// expression's type. `input` indexes into
    /// [`crate::graph::MirGraph::ops`] in topological order.
    Extend {
        input: usize,
        field: SmolStr,
        expr: Expr<'db>,
    },

    /// `RenameOp(input, old, new)` — rename a column.
    Rename {
        input: usize,
        old: SmolStr,
        new: SmolStr,
    },

    /// `FilterOp(input, pred)` — keep rows satisfying the boolean predicate.
    Filter { input: usize, pred: Expr<'db> },

    /// `JoinOp(left, right, on, kind)` — relational join. Each side carries the
    /// name its columns are addressed by; see [`JoinSide`].
    Join {
        left: JoinSide,
        right: JoinSide,
        on: Expr<'db>,
        kind: JoinKind,
    },

    /// `UnionOp(left, right, relation)` — multiset union of two same-schema
    /// streams, under the name the unified row answers to.
    ///
    /// `relation` is there for the same reason [`JoinSide::relation`] is: a
    /// column reference carries the relation it belongs to, and after a union
    /// neither input's name is that relation. `Staff.union(Contractor)` yields
    /// rows from both, so addressing them as `Staff.email` would name the row
    /// by whichever side happened to be written first — the ambiguity the
    /// qualified `select` was decided against on 2026-08-14. Both sides are
    /// re-qualified under this name and the body addresses the pipeline.
    Union {
        left: usize,
        right: usize,
        relation: SmolStr,
    },

    /// `GroupByOp(input, keys, aggs, relation)` — one row per distinct
    /// combination of the key columns, carrying the aggregates of each group.
    ///
    /// **It was two operators and neither could stand alone.** `GroupBy {
    /// input, keys }` with nothing aggregated is [`Op::Project`] followed by
    /// [`Op::Distinct`], which the surface already spells twice over;
    /// `Aggregate { input, aggs }` with no grouping has no groups. `schema_of`
    /// had to read the pair to describe either, `DataFusion` takes both halves in
    /// one `aggregate(group_expr, aggr_expr)` call, and nothing in the
    /// workspace ever constructed either of them.
    ///
    /// `keys` are QUALIFIED, like [`Op::Project`]'s columns. They were bare
    /// `SmolStr`s, from before the join stopped flattening its two sides: after
    /// a join, a bare `id` names whichever side `DataFusion` finds first, and the
    /// ambiguity is the one the qualified `select` was decided against on
    /// 2026-08-14.
    ///
    /// `relation` is what the AGGREGATE columns answer to, for the reason
    /// [`Op::Union`] carries one: a group's total came from no side, so there
    /// is nothing to borrow a qualifier from. The keys keep their own.
    GroupBy {
        input: usize,
        keys: Vec<ProjectedColumn>,
        aggs: Vec<AggSpec<'db>>,
        relation: SmolStr,
    },

    /// `DistinctOp(input)` — deduplicate whole rows.
    ///
    /// It carried an optional `by: Vec<SmolStr>` for `DISTINCT ON`, which
    /// nothing ever constructed. `DISTINCT ON` picks one row per group and
    /// which one is arbitrary without an `ORDER BY`, so the language cannot
    /// admit it while `sort` has no lowering: a conformance artefact blessed
    /// byte-for-byte would differ between two runs of the same program. See
    /// `/docs/design/discarded`.
    Distinct { input: usize },

    /// `EmitVertex(input, type_name, rdf_type?, id, dedup, props)` — project rows
    /// to a typed property-graph VERTEX. The property-graph-canonical model: one
    /// `EmitVertex` per shape carrying ALL its columns (map-only wide-row). The
    /// backend assigns the dense vertex id and materialises `GraphAr`; the PG model
    /// lives here, not in triples. `id` is the subject IRI expression (typically a
    /// `Concat` template); `dedup` collapses duplicate ids (single-valued shape).
    EmitVertex {
        input: usize,
        type_name: SmolStr,
        rdf_type: Option<SmolStr>,
        id: Expr<'db>,
        dedup: bool,
        props: Vec<VProp<'db>>,
    },

    /// `EmitEdge(input, edge_type, rdf_uri?, src_type, dst_type, src_id, dst_id,
    /// single_valued)` — project rows to a property-graph EDGE. `src_id` / `dst_id`
    /// are the endpoint subject IRIs; the backend resolves them to dense vertex ids
    /// (join against the vertex tables) and emits the CSR/CSC adjacency files.
    /// `single_valued` (shape cardinality) chooses one-edge-per-source vs keep-all.
    EmitEdge {
        input: usize,
        edge_type: SmolStr,
        rdf_uri: Option<SmolStr>,
        src_type: SmolStr,
        dst_type: SmolStr,
        src_id: Expr<'db>,
        dst_id: Expr<'db>,
        single_valued: bool,
    },

    /// `SinkOp(input, sink)` — terminal node; no operator may consume a `Sink`
    /// output.
    Sink { input: usize, sink: SinkRef },

    /// `Empty(schema)` — the empty relation. Carries the column schema
    /// it would have produced so codegen can emit a `SELECT ... WHERE false`
    /// (or `LIMIT 0`) shell of the right shape. It is a distinct variant rather
    /// than a `Source` carrying an empty marker because a `Source` that yields
    /// no rows is a special case every downstream operator would have to reason
    /// about; a variant of its own is self-documenting and keeps `schema_of`
    /// total.
    Empty { schema: Vec<SmolStr> },
}

/// One column an [`Op::Project`] keeps, under the relation that owns it —
/// `source` and `column`, the same pair [`Expr::ColRef`] carries and spelled the
/// same way, because it is the same pair.
///
/// It was a bare [`SmolStr`]. `HirSourceOp::Select` had lost the qualification
/// before the lowering ever saw it, so there was nothing to carry; open question
/// 4 of `grammar.bnf, § OPEN` was decided on 2026-08-14 (`select` may follow a
/// `join` and names a QUALIFIED column) and the binding now reaches here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ProjectedColumn {
    /// The relation the column belongs to — `Employee` in
    /// `Active.select(Employee.id)`.
    pub source: SmolStr,
    /// The column itself — `id`.
    pub column: SmolStr,
}

/// One side of an [`Op::Join`] — the relation it reads and the names that
/// relation's columns are addressed by.
///
/// It was four flat fields, `left`/`right` beside `left_name`/`right_name`, and
/// keeping the index next to its own name is the same reason
/// [`ProjectedColumn`] is a struct: nobody recovers the pairing from a call
/// site.
///
/// # Why `relations` is a set and was a name
///
/// **A side is addressed by every qualifier its columns actually carry, and a
/// join produces two.** This field was one `SmolStr`, and the lowering filled it
/// with the pipeline's own name after a join — a name that qualifies NOTHING in
/// the relation, because [`Op::Join`] is the one composite operator that does
/// not re-qualify. [`Op::Union`] does (its schema comes back from `DataFusion`
/// with no qualifier at all, so there is nothing to keep) and [`Op::GroupBy`]
/// does it to the aggregates; a join keeps both sides' qualifiers side by side,
/// which is the whole of why the body may write `Purchase.amount` and
/// `User.email` after one.
///
/// The one name was therefore not a narrower truth but a false one, and it made
/// two programs that pass `fossil check` fail to plan:
///
/// - `Both := L.join(R, …)` then `Tri := Both.join(T, on = R.k == T.k)` — the
///   engine refused `R` as "neither input (`Both`, `T`)" while `Both` qualified
///   no column of either;
/// - `Purchase.join(Adults, on = Purchase.user_id == User.id)`, which the
///   paragraph below has described as working since it was written.
///
/// The invariant this field states is checkable against the backend by
/// induction over `fossil_df::plan::build`: a `Source` is qualified by its
/// binding, `Filter`/`Distinct` preserve, `Project` keeps the qualifier each
/// column was written with, `Union` collapses to the one name it carries,
/// `GroupBy` keeps its keys' and adds its own for the aggregates, and `Join`
/// unions the two sides'.
///
/// `alias` is `Node.join(Node as Other, …)` — the second name for the same
/// source, and the only thing that can tell the two sides of a self-join apart.
/// It REPLACES `relations` rather than sitting beside them (mirroring
/// `fossil_hir::infer::RowScope::rename_to`, where the alias collapses the right
/// scope to one row), so the backend re-qualifies that side under it and the
/// side is addressed by the alias ALONE. A side with no alias keeps the
/// qualification it already carries — which is why this is `Option` and not just
/// a name: `Purchase.join(Adults, …)` reads a pipeline called `Adults` whose
/// columns are addressed as `User.…`, and re-qualifying it under `Adults` would
/// rename exactly the columns the body refers to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct JoinSide {
    /// Index into the op list of the operator producing this side's relation.
    pub input: usize,
    /// Every name this side's columns are already qualified by — one source
    /// binding, or the bindings a pipeline's own joins left addressable.
    pub relations: Vec<SmolStr>,
    /// The `X as Y` of a self-join, when one was written.
    pub alias: Option<SmolStr>,
}

impl JoinSide {
    /// The names the body may address this side's columns by — the alias alone
    /// when there is one, every relation it carries otherwise.
    #[must_use]
    pub fn names(&self) -> &[SmolStr] {
        match &self.alias {
            Some(alias) => std::slice::from_ref(alias),
            None => &self.relations,
        }
    }

    /// Whether `name` qualifies a column of this side.
    #[must_use]
    pub fn addresses(&self, name: &str) -> bool {
        self.names().iter().any(|n| n == name)
    }
}

/// Relational join flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum JoinKind {
    Inner,
    LeftOuter,
    RightOuter,
    Full,
}

/// One aggregation of an [`Op::GroupBy`] — `agg_fn(column) AS out_field`, the
/// result typed `ty`.
///
/// `column` is qualified for the same reason [`Op::GroupBy`]'s keys are: it was
/// a bare `in_field`, and a bare name after a join names a side by accident.
/// `ty` is the AGGREGATE's type and not the column's — that is the whole of why
/// this verb is checker work: `math.sum` takes a `Float` and gives a `Float`,
/// so summing an `Integer` column produces a `Float` one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AggSpec<'db> {
    pub out_field: SmolStr,
    pub agg_fn: AggFn,
    pub column: ProjectedColumn,
    pub ty: Ty<'db>,
}

/// One property (column) of an [`Op::EmitVertex`] — `value AS name`, typed `ty`.
///
/// The PG-canonical replacement for a per-predicate triple emission: a vertex's
/// properties are carried together so the backend emits one wide row per source
/// row. `ty` is fossil's canonical type — the backend derives the `GraphAr`/xsd
/// spelling from it (the core stays format-agnostic). `rdf_uri` is the predicate
/// IRI the manifest/DCAT layer reads; `single_valued` (shape cardinality) drives
/// duplicate collapse.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct VProp<'db> {
    pub name: SmolStr,
    pub value: Expr<'db>,
    pub ty: Ty<'db>,
    pub rdf_uri: Option<SmolStr>,
    pub single_valued: bool,
}

// `AggFn` lived here, with a `Count` nothing could construct. It is
// `fossil_hir::stdlib::AggFn` now: which calls are aggregates is the
// CATALOGUE's answer, and a copy of the answer on this side of the lowering
// would be a second table to keep in step with four rows of `math/`.

/// Source formats.
///
/// The three native `io/` constructors map to a `DuckDB` table
/// function in codegen (`read_csv_auto` / `read_json_auto` / `read_parquet`)
/// that runs identically on native DuckDB and DuckDB-WASM.
///
/// [`Provider`](Self::Provider) covers formats DuckDB can't read natively: the
/// core stays format-agnostic — codegen scans a relation an external provider
/// materialises, and the decode lives outside the core. Carrying the provider
/// name makes this non-`Copy`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
#[allow(clippy::doc_markdown)] // read_csv_auto/read_json_auto/read_parquet are SQL fn names
pub enum SourceFormat {
    /// `io.csv(...)` → `read_csv_auto`.
    Csv,
    /// `io.json(...)` → `read_json_auto`.
    Json,
    /// `io.parquet(...)` → `read_parquet`.
    Parquet,
    /// A format DuckDB cannot read natively (e.g. RDF), backed by an external
    /// source provider named `name` (`io.<name>(...)`). Codegen scans the
    /// relation the provider materialises; the runtime invokes the provider
    /// before the source prelude. The core never sees the format's internals.
    Provider { name: SmolStr },
}

/// Sink references. `GraphAr` is the only one built; `Turtle`, `JsonLd` and
/// `NQuads` are the serialisations this enum is shaped to take next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum SinkRef {
    GraphAr,
}

/// Typed MIR expression.
///
/// It replaced an untyped `ExprLowered`. `LitString` / `ColRef` / `Concat` are
/// the rendering-compatible subset — the SQL for `examples/hello.fossil` is
/// byte-identical across the change.
///
/// The `ty: Ty<'db>` carriage on `Call` / `BinOp` is intentional — it makes
/// the "erase types ≡ untyped property" check testable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Expr<'db> {
    /// Literal string (e.g. `"https://example.org/user/"`).
    LitString(SmolStr),
    /// Literal boolean. Renders `TRUE` / `FALSE`.
    LitBool(bool),
    /// Literal integer (`18`).
    LitInt(i64),
    /// Literal float (`0.5`), by its bits — see `fossil_hir::FloatBits`.
    LitFloat(fossil_hir::FloatBits),
    /// Column reference (e.g. `users.id`).
    ColRef { source: SmolStr, column: SmolStr },
    /// String concatenation: `lhs || rhs`. Recursive via `Box` so the variant
    /// has a finite size; `salsa::Update` lifts through `Box<T>` when
    /// `T: Update`.
    Concat(Box<Expr<'db>>, Box<Expr<'db>>),
    /// `x IS NULL` / `x IS NOT NULL` — what `x == null` and `x != null` lower
    /// to, and the reason there is no null VALUE in this algebra.
    ///
    /// **`x != NULL` is not true in SQL, it is NULL**, so a filter written as
    /// an ordinary comparison against a null literal keeps no rows at all. The
    /// surface has one spelling and the algebra has the operator that means it;
    /// `null` never survives as a value, which is the same statement the
    /// checker makes by refusing `name = null`.
    IsNull {
        operand: Box<Expr<'db>>,
        /// `true` for `!=` — the `NOT` of `IS NOT NULL`.
        negated: bool,
    },
    /// Function application: `func(args...)`, result typed `ty`.
    Call {
        func: SmolStr,
        args: Vec<Expr<'db>>,
        ty: Ty<'db>,
    },
    /// Binary operator: `lhs <op> rhs`, result typed `ty`.
    BinOp {
        op: BinOp,
        lhs: Box<Expr<'db>>,
        rhs: Box<Expr<'db>>,
        ty: Ty<'db>,
    },
    /// Unary operator: `-operand` or `not operand`, result typed `ty`.
    ///
    /// It survives lowering as itself rather than becoming `0 - operand` /
    /// `operand == FALSE`, and the reason is in `fossil_hir::HirExpr::UnaryOp`:
    /// `0.0 - 0.0` is `+0.0` where `-(0.0)` is `-0.0`, so the rewrite changes
    /// the bits a `xsd:float` column carries.
    UnaryOp {
        op: UnOp,
        operand: Box<Expr<'db>>,
        ty: Ty<'db>,
    },
    /// `cond ? then : otherwise`, result typed `ty`. Renders as a two-armed
    /// `CASE` on every SQL engine; both branches have the same type by
    /// construction (the checker refuses anything else), so the CASE is total.
    Ternary {
        cond: Box<Expr<'db>>,
        then: Box<Expr<'db>>,
        otherwise: Box<Expr<'db>>,
        ty: Ty<'db>,
    },
    /// Named runtime assertion. `span_line` carries the source line for the
    /// diagnostic, so a constraint that fails at run time names the line that
    /// wrote it rather than a column of the generated SQL.
    Assert {
        name: SmolStr,
        span_line: u32,
        inner: Box<Expr<'db>>,
    },
}
