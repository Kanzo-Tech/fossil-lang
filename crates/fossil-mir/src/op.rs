//! Typed operator algebra — the complete 11-operator typed set.
//!
//! Phase 4 (CORE-08..10) extends the Phase 1 4-variant `Op<'db>` to all 11
//! operators of `operator-algebra.md` §2 (`Source`, `Project`, `Extend`,
//! `Rename`, `Filter`, `Join`, `Union`, `GroupBy`, `Aggregate`, `Distinct`,
//! `TripleEmit`, `Sink`) plus an [`Op::Empty`] node (the R9 empty-source
//! representation — see ADR-0009), and replaces the thin untyped `ExprLowered`
//! with a typed [`Expr`] ADT carrying [`Ty`] on the synthesised nodes.
//!
//! See `operator-algebra.md` for the full algebra spec and ADR-0009 for the
//! reachability decision (all 11 defined; only `Source` / `Extend` /
//! `TripleEmit` / `Sink` are lowered from `.fossil` source this phase, the
//! other 7 are exercised via direct `MirGraph` construction in plans
//! 04-04/04-05).
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
//!   is fine and makes the "erase types ≡ untyped algebra" property (SC#3,
//!   plan 04-07) testable.
//! - `input` / `left` / `right` are `usize` indices into
//!   [`crate::graph::MirGraph::ops`] in topological order. Two-input ops carry
//!   two indices.

use fossil_hir::Ty;
use smol_str::SmolStr;

/// One node of the MIR DAG. The complete typed operator algebra:
/// `operator-algebra.md` §2's 9 operators + the two typed-sink refinements
/// (`TripleEmit`, `Sink`) + [`Op::Empty`] (R9 target).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Op<'db> {
    /// `SourceOp(uri, format, row_type)` — origin of all row data
    /// (operator-algebra.md §2.1).
    ///
    /// `row_type` is the `Record` row type derived from the descriptor (CSVW)
    /// or the typed source row (Phase 3 `TypeckOutput.source_row`).
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

    /// `ProjectOp(input, cols)` — restrict the row to the selected columns
    /// (operator-algebra.md §2.2).
    Project { input: usize, cols: Vec<SmolStr> },

    /// `ExtendOp(input, field, expr)` — add a computed field whose type is the
    /// expression's type (operator-algebra.md §2.3). `input` indexes into
    /// [`crate::graph::MirGraph::ops`] in topological order.
    Extend {
        input: usize,
        field: SmolStr,
        expr: Expr<'db>,
    },

    /// `RenameOp(input, old, new)` — rename a column (operator-algebra.md §2.4).
    Rename {
        input: usize,
        old: SmolStr,
        new: SmolStr,
    },

    /// `FilterOp(input, pred)` — keep rows satisfying the boolean predicate
    /// (operator-algebra.md §2.5).
    Filter { input: usize, pred: Expr<'db> },

    /// `JoinOp(left, right, on, kind)` — relational join
    /// (operator-algebra.md §2.6). `left_name` / `right_name` qualify the two
    /// input streams for collision-safe schema union.
    Join {
        left: usize,
        right: usize,
        on: Expr<'db>,
        kind: JoinKind,
        left_name: SmolStr,
        right_name: SmolStr,
    },

    /// `UnionOp(left, right)` — multiset union of two same-schema streams
    /// (operator-algebra.md §2.7).
    Union { left: usize, right: usize },

    /// `GroupByOp(input, keys)` — group rows by the key columns
    /// (operator-algebra.md §2.8).
    GroupBy { input: usize, keys: Vec<SmolStr> },

    /// `AggregateOp(input, aggs)` — aggregate grouped rows
    /// (operator-algebra.md §2.9).
    Aggregate {
        input: usize,
        aggs: Vec<AggSpec<'db>>,
    },

    /// `DistinctOp(input, by?)` — deduplicate rows, optionally by a subset of
    /// columns (operator-algebra.md §2.10).
    Distinct {
        input: usize,
        by: Option<Vec<SmolStr>>,
    },

    /// `TripleEmitOp(input, subject, predicate, object, graph?)` — emit one RDF
    /// triple per row (operator-algebra.md §2.11, typed-sink refinement).
    ///
    /// Phase 1 used `subject_col` / `object_col: SmolStr`; Phase 4 generalises
    /// both to [`Expr`] (a bare `ColRef` / `Concat` renders byte-identically).
    /// `graph` is the optional named-graph IRI (RDF 1.2 quad).
    TripleEmit {
        input: usize,
        subject: Expr<'db>,
        predicate: SmolStr,
        object: Expr<'db>,
        graph: Option<SmolStr>,
    },

    /// `SinkOp(input, sink)` — terminal node; no operator may consume a `Sink`
    /// output (operator-algebra.md §2.12, typed-sink refinement).
    Sink { input: usize, sink: SinkRef },

    /// `Empty(schema)` — the R9 empty-source target. Carries the column schema
    /// it would have produced so codegen can emit a `SELECT ... WHERE false`
    /// (or `LIMIT 0`) shell of the right shape. See ADR-0009 for why this is a
    /// distinct variant rather than a `Source` with an empty marker.
    Empty { schema: Vec<SmolStr> },
}

/// Relational join flavour (operator-algebra.md §2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum JoinKind {
    Inner,
    LeftOuter,
    RightOuter,
    Full,
}

/// One aggregation in an [`Op::Aggregate`] — `agg_fn(in_field) AS out_field`,
/// the result typed `ty` (operator-algebra.md §2.9).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AggSpec<'db> {
    pub out_field: SmolStr,
    pub agg_fn: AggFn,
    pub in_field: SmolStr,
    pub ty: Ty<'db>,
}

/// Aggregation function (operator-algebra.md §2.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum AggFn {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

/// Source formats.
///
/// The three native `io/` constructors (STDL-06) map to a `DuckDB` table
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

/// Sink references. Phase 1 ships `GraphAr`; Phase 9+ adds `Turtle`, `JsonLd`,
/// `NQuads`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum SinkRef {
    GraphAr,
}

/// Typed MIR expression.
///
/// The Phase 4 generalisation of the Phase 1 `ExprLowered`. `LitString` /
/// `ColRef` / `Concat` are the rendering-compatible subset (hello.fossil SQL is
/// byte-identical); `Call` / `BinOp` / `Assert` / `LitBool` are the Phase 4..6
/// additions.
///
/// The `ty: Ty<'db>` carriage on `Call` / `BinOp` is intentional — it makes
/// the "erase types ≡ untyped property" check (SC#3, plan 04-07) testable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Expr<'db> {
    /// Literal string (e.g. `"https://example.org/user/"`).
    LitString(SmolStr),
    /// Literal boolean. Renders `TRUE` / `FALSE`.
    LitBool(bool),
    /// Column reference (e.g. `users.id`).
    ColRef { source: SmolStr, column: SmolStr },
    /// String concatenation: `lhs || rhs`. Recursive via `Box` so the variant
    /// has a finite size; `salsa::Update` lifts through `Box<T>` when
    /// `T: Update`.
    Concat(Box<Expr<'db>>, Box<Expr<'db>>),
    /// Function application: `func(args...)`, result typed `ty`. The stdlib
    /// function → SQL mapping lands in Phase 5; Phase 4 renders a passthrough
    /// `func(arg, ...)` call.
    Call {
        func: SmolStr,
        args: Vec<Expr<'db>>,
        ty: Ty<'db>,
    },
    /// Binary operator: `lhs <op> rhs`, result typed `ty`.
    BinOp {
        op: CmpOp,
        lhs: Box<Expr<'db>>,
        rhs: Box<Expr<'db>>,
        ty: Ty<'db>,
    },
    /// Named runtime assertion (R7-R10). `span_line` carries the source line for
    /// the diagnostic. Populated by plan 04-06; until then codegen renders the
    /// `inner` expression (no-op wrapper).
    Assert {
        name: SmolStr,
        span_line: u32,
        inner: Box<Expr<'db>>,
    },
}

/// Comparison / boolean operator for [`Expr::BinOp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}
