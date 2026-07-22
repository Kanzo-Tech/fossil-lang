//! [`MirGraph`] — the Salsa-tracked container for a MIR DAG.
//!
//! The list is stored in topological order; an `Op::Extend { input: 0, .. }`
//! refers to `ops[0]`. [`crate::lower::lower_to_mir_pg`] keeps this invariant
//! trivially because it emits ops in the only valid order; the R1-R10 rewriting
//! preserves it.

use crate::op::Op;

#[salsa::tracked(debug)]
pub struct MirGraph<'db> {
    /// Operators in topological order. `Op::Extend { input: 0, .. }` refers
    /// to `ops[0]`.
    #[returns(ref)]
    pub ops: Vec<Op<'db>>,

    /// Poison marker: lowering could not resolve something the graph needs (an
    /// unresolvable source binding, a mapping with no usable `iri`).
    ///
    /// Lowering MUST NOT substitute a plausible default for something it failed
    /// to resolve — that turns a compile error into wrong output, silently, and
    /// is exactly how a mapping over `@upv/aemet.csv` ended up reading
    /// `examples/users.csv`. It also must not panic: `lower_to_mir_pg` runs on
    /// every keystroke from the LSP, over half-written programs. So it taints
    /// instead, following rustc's `ErrorGuaranteed` discipline — the marker's
    /// presence implies ≥1 accumulated `Diagnostic` (`fossil_base` P-CRIT-4), so
    /// the editor shows the error while consumers refuse the graph.
    ///
    /// Consumers MUST check this before executing; `fossil_df::execute_graph` is
    /// the enforcement point.
    pub error: Option<fossil_base::ErrorGuaranteed>,
}
