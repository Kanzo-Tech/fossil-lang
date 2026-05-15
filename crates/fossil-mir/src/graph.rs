//! [`MirGraph`] — the Salsa-tracked container for a MIR DAG.
//!
//! Phase 1 has at most 4 nodes per mapping (`Source`, `Extend`, `TripleEmit`, `Sink`).
//! Phase 4 (CORE-08..10) grows this to the full 11-operator algebra plus
//! intermediate ops introduced by R1-R10 rewriting.
//!
//! The list is stored in topological order; an `Op::Extend { input: 0, .. }`
//! refers to `ops[0]`. Phase 1 keeps this invariant trivially because
//! [`crate::lower::lower_to_mir`] emits ops in the only valid order. Phase 4
//! will introduce a topo-sort pass after rewriting.

use crate::op::Op;

#[salsa::tracked(debug)]
pub struct MirGraph<'db> {
    /// Operators in topological order. `Op::Extend { input: 0, .. }` refers
    /// to `ops[0]`. Phase 1 emits at most 4 entries.
    #[returns(ref)]
    pub ops: Vec<Op<'db>>,
}
