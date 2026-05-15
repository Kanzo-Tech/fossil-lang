//! Typed operator algebra — Phase 1 ships 3 of 11 operators + a Sink terminal.
//!
//! Phase 4 (CORE-08..10) extends to all 11 operators (`Project`, `Rename`,
//! `Filter`, `Join`, `Union`, `GroupBy`, `Aggregate`, `Distinct`) and wires
//! the R1-R10 rewriting pass on this `Op<'db>` enum. The variant set
//! documented here is locked for Phase 1; Phase 4 only ADDS variants.
//!
//! See `operator-algebra.md` for the full algebra spec.
//!
//! # Design notes (Phase 1)
//!
//! - `ExprLowered::Concat` wraps recursion via `Box<ExprLowered>`. The
//!   `salsa::Update` derive handles `Box<T>` transparently when `T: Update`,
//!   so no interned-newtype escape hatch is needed (cf. the `Record<'db>`
//!   pattern from `fossil-hir`). Phase 4 generalises `ExprLowered` to a full
//!   typed `Expr` ADT.
//! - `Op<'db>` carries `'db` because the `Source` variant references
//!   `Ty<'db>` (an interned handle). `Extend` / `TripleEmit` / `Sink` are
//!   `'db`-free in their data, but the enum-level lifetime is required by the
//!   `Record(Record<'db>)` reference inside `Source.row_type`.

use fossil_hir::Ty;
use smol_str::SmolStr;

/// One node of the MIR DAG. Phase 1 ships 3 typed operators (`Source`,
/// `Extend`, `TripleEmit`) and the `Sink` terminal; Phase 4 (CORE-08..10)
/// adds the remaining 8.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum Op<'db> {
    /// `SourceOp(uri, format, row_type)`.
    ///
    /// Phase 1: `format` is always `Csv`; `row_type` is hardcoded
    /// `Record({id: String, name: String})` (Phase 3 CORE-05 derives it from
    /// the CSVW descriptor).
    Source {
        uri: SmolStr,
        format: SourceFormat,
        row_type: Ty<'db>,
    },

    /// `ExtendOp(input_idx, field, expr)`.
    ///
    /// Phase 1 uses this to attach the IRI-template result as a column on the
    /// upstream stream. `input` indexes into [`crate::graph::MirGraph::ops`]
    /// in topological order.
    Extend {
        /// Index of the upstream op in the [`crate::graph::MirGraph`] ops list.
        input: usize,
        /// Output column name.
        field: SmolStr,
        /// Lowered expression producing the new column's values.
        expr: ExprLowered,
    },

    /// `TripleEmitOp(input_idx, subject_col, predicate_iri, object_col)`.
    ///
    /// Phase 1: `predicate` is a static IRI; `subject_col` references a
    /// column produced upstream (typically by an `Extend`); `object_col`
    /// references a column from the `Source` row type.
    TripleEmit {
        input: usize,
        subject_col: SmolStr,
        predicate: SmolStr,
        object_col: SmolStr,
    },

    /// `SinkOp(input_idx, sink)`. Terminal node; no operator may consume a
    /// `Sink` output.
    Sink { input: usize, sink: SinkRef },
}

/// Phase 1 source formats. Phase 5 adds `Json`, `Parquet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum SourceFormat {
    Csv,
}

/// Phase 1 sink references. Phase 9+ adds `Turtle`, `JsonLd`, `NQuads`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum SinkRef {
    GraphAr,
}

/// Phase 1 lowered expression — supports concatenation of literal strings
/// and column references. Phase 4 generalises to a full typed `Expr` ADT.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ExprLowered {
    /// Literal string (e.g. `"https://example.org/user/"`).
    LitString(SmolStr),
    /// Column reference (e.g. `users.id`).
    ColRef { source: SmolStr, column: SmolStr },
    /// String concatenation: `lhs || rhs`. Recursive via `Box` so the variant
    /// has a finite size; `salsa::Update` lifts through `Box<T>` when
    /// `T: Update`, so no interned-newtype escape hatch is required.
    Concat(Box<ExprLowered>, Box<ExprLowered>),
}
