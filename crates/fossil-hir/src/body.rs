//! [`HirBody`] — per-mapping body content + the [`body`] Salsa query.
//!
//! Lower half of the CORE-02 invalidation-barrier pattern (see ADR-0005).
//! [`crate::item_tree::ItemTree`] is the SIGNATURE table; this module is the
//! BODY table. They are deliberately separate `#[salsa::tracked]` queries
//! with separate input-dependency surfaces.
//!
//! - `item_tree(db, file)` reads top-level header tokens + structural counts
//!   only.
//! - `body(db, mapping)` reads the body of ONE mapping.
//!
//! Editing one mapping's body invalidates exactly that mapping's
//! `body(M_k)` (and its downstream type-check / MIR / codegen queries).
//! Sibling mappings stay cached.
//!
//! # `nth(idx)` correctness — CRITICAL
//!
//! [`body`] resolves the target mapping via [`mapping_cst_node`]'s filter-
//! before-nth lookup over MAPPING-kind CST children. `.filter(...)` MUST
//! come BEFORE `.nth(idx)`: without it, `PREFIX_DECL` / `SOURCE_DEF` /
//! `IMPORT` top-level children inflate the count and `.nth(idx)` returns the
//! wrong node. The regression test
//! [`tests::body_filters_to_mapping_kind_before_indexing`] enforces this.
//!
//! # CORE-02 SC#2 per-mapping invalidation barrier — CRITICAL
//!
//! [`body`] does NOT depend on `fossil_syntax::parse(db, file)` directly.
//! Doing so would tie every per-mapping body query to the whole file's CST,
//! causing all 10 body queries in a 10-mapping file to re-execute on any
//! body edit (because `parse()` returns a new `Cst` tracked struct whose
//! `root: CstRoot` field changes structurally on any byte edit). The
//! invalidation regression test
//! `crates/fossil-hir/tests/invalidation_regression.rs` exists exactly to
//! catch this regression.
//!
//! The fix: [`mapping_cst_node`] is an intermediate per-mapping CST-
//! extraction Salsa query that returns just the green subtree for one
//! MAPPING. Because rowan's `GreenNode` interner reuses subtree Arcs
//! across edits to UNAFFECTED siblings, the green subtree for mapping #1
//! is bit-identical (same Arc) before and after an edit to mapping #3.
//! Salsa's structural equality check on [`MappingCstNode`] (which
//! delegates to `GreenNode::eq` → rowan structural-tree equality →
//! Arc-pointer equality on shared subtrees) returns "no change", so
//! downstream [`body`] queries for unaffected mappings validate via
//! `DidValidateMemoizedValue` instead of re-executing.
//!
//! This is the rust-analyzer per-item Salsa fan-out pattern; the
//! intermediate query is the data-layout fix per ADR-0005's structural
//! invariant.

use crate::def_map::{MappingLoc, def_map};
use crate::lower::{HirProperty, lower_property_public};
use fossil_syntax::{SyntaxKind, SyntaxNode};
use rowan::GreenNode;

/// Stable per-mapping expression id. Indexed into the body's expression
/// arena. Phase 2 plan 02-06 wires the per-mapping provenance side table
/// keyed by `(MappingLoc, ExprId)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ExprId(pub u32);

/// Per-mapping body content.
///
/// Owns the flat `Vec<HirProperty>` that Phase 1's `HirMapping.properties`
/// previously carried (the field is REMOVED from `HirMapping` per
/// ADR-0005). The [`Self::expr_count`] is a Phase 2 placeholder for plan
/// 02-06's `ExprId` arena bookkeeping.
#[salsa::tracked(debug)]
pub struct HirBody<'db> {
    /// Per-mapping flat property list, in source order.
    #[returns(ref)]
    pub properties: Vec<HirProperty>,
    /// Count of distinct expression nodes lowered for this mapping. Phase 2
    /// plan 02-06 keys the provenance side table by `(MappingLoc, ExprId)`
    /// for IDs in `0..expr_count`.
    pub expr_count: u32,
}

/// Salsa-storable handle to a per-mapping CST subtree.
///
/// Holds just the MAPPING node, not the whole file. Wraps a
/// [`rowan::GreenNode`] so the [`mapping_cst_node`] Salsa query can store
/// and structurally compare per-mapping subtrees.
///
/// Why this exists: editing one mapping's body changes the WHOLE file's
/// `Cst` (because rowan rebuilds the root green node up the spine to the
/// edit site). If [`body`] depended on `parse(db, file)`'s `Cst` directly,
/// EVERY per-mapping body query would see a different `Cst` input and
/// re-execute. By inserting [`mapping_cst_node`] between `parse` and `body`,
/// per-mapping bodies depend on a Salsa-tracked value that's STRUCTURALLY
/// EQUAL for sibling mappings (rowan reuses subtree `Arc`s across edits to
/// unaffected siblings; the structural-equality check on `GreenNode`
/// returns `true` even if the Arcs aren't pointer-equal). Salsa's
/// `maybe_update` returns `false` for unchanged values, so downstream
/// queries validate via `DidValidateMemoizedValue` instead of re-executing.
///
/// This is the rust-analyzer per-item Salsa fan-out pattern. The invariant
/// is enforced by the CORE-02 SC#2 regression test at
/// `crates/fossil-hir/tests/invalidation_regression.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MappingCstNode {
    /// The per-mapping green node, or `None` if the mapping index didn't
    /// resolve to a MAPPING-kind CST child.
    green: Option<GreenNode>,
}

// SAFETY: third-party-trait integration boundary (per ADR-0004). Salsa's
// `Update` trait is `unsafe` by design — implementations must guarantee
// `maybe_update` correctly determines whether the new value differs. We
// delegate to `PartialEq` on `Option<GreenNode>`, which rowan implements
// as structural tree equality on the inner node. No safe alternative
// because Salsa requires `unsafe impl` even for trivially-safe bodies.
#[allow(unsafe_code)]
unsafe impl salsa::Update for MappingCstNode {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: caller guarantees `old_pointer` is a valid, aligned
        // pointer to an initialised `MappingCstNode` owned by Salsa storage
        // (Salsa contract).
        let old = unsafe { &mut *old_pointer };
        if *old == new_value {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

impl MappingCstNode {
    /// Reconstruct the red [`SyntaxNode`] view from the stored green node.
    /// Returns `None` if the mapping index didn't resolve.
    #[must_use]
    pub fn syntax(&self) -> Option<SyntaxNode> {
        self.green.as_ref().map(|g| SyntaxNode::new_root(g.clone()))
    }
}

/// Per-mapping CST extraction query. Resolves a [`MappingLoc`] to its
/// owned [`GreenNode`] subtree (just the MAPPING node, not the whole file).
///
/// This is the invalidation barrier between `parse(db, file)` (which
/// re-executes on any byte edit) and [`body`] (which depends only on the
/// per-mapping CST subtree, stable across sibling-mapping edits).
///
/// Resolution uses filter-then-nth over MAPPING-kind top-level children
/// (per ADR-0005 + plan 02-04 contract; matches `def_map`'s per-MAPPING-
/// kind dense indexing).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn mapping_cst_node<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> MappingCstNode {
    let file = mapping.file(db);
    let idx = mapping.index(db);
    let cst = fossil_syntax::parse(db, file);

    // CRITICAL: filter to MAPPING-kind BEFORE `.nth(idx)`. See the module-
    // level comment + the `body_filters_to_mapping_kind_before_indexing`
    // regression test. The previous (buggy) form was
    //   cst.root(db).syntax().children().nth(idx).filter(|n| n.kind() == MAPPING)
    // which silently returns the wrong node when a file has non-MAPPING
    // top-level siblings before the target mapping.
    let mapping_node = cst
        .root(db)
        .syntax()
        .children()
        .filter(|n| n.kind() == SyntaxKind::MAPPING)
        .nth(idx);

    let green = mapping_node.map(|n| n.green().into_owned());
    MappingCstNode { green }
}

/// Per-mapping body query. Keyed by the interned [`MappingLoc`]; depends on
/// the per-mapping CST subtree (via [`mapping_cst_node`]) and the file's
/// prefix table (via [`def_map`]). Body content is read from EXACTLY ONE
/// mapping — editing a sibling mapping's body does NOT invalidate
/// `body(M_k)` because [`mapping_cst_node`] is the invalidation barrier
/// (rowan's subtree Arc reuse → structural equality → Salsa
/// `DidValidateMemoizedValue` instead of re-execution).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn body<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> HirBody<'db> {
    let file = mapping.file(db);
    let dm = def_map(db, file);
    let prefixes = dm.prefixes(db);
    let mapping_cst = mapping_cst_node(db, mapping);

    let mut properties: Vec<HirProperty> = Vec::new();
    let mut expr_count: u32 = 0;
    if let Some(node) = mapping_cst.syntax()
        && let Some(body_node) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)
    {
        for prop_node in body_node
            .children()
            .filter(|c| c.kind() == SyntaxKind::PROPERTY)
        {
            if let Some(prop) = lower_property_public(db, &prop_node, prefixes) {
                properties.push(prop);
                expr_count = expr_count.saturating_add(1);
            }
        }
    }
    HirBody::new(db, properties, expr_count)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

    #[test]
    fn body_returns_two_properties_for_hello() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
        let dm = def_map(&db, file);
        let m = dm.mappings(&db)[0];
        let b = body(&db, m);
        assert_eq!(b.properties(&db).len(), 2);
        assert_eq!(b.expr_count(&db), 2);
    }

    /// Regression test for the wrong-body bug (plan-checker Blocker 2).
    ///
    /// A file with `prefix + source_def + 3 mappings` MUST return the right
    /// mapping for each `body(M_i)` call. The three mappings have distinct
    /// property counts (3, 2, 1), so an off-by-N indexing bug surfaces as
    /// the wrong property count. Specifically:
    ///
    /// - `body(M_0)` returns `Mapping_A` (3 properties)
    /// - `body(M_1)` returns `Mapping_B` (2 properties)
    /// - `body(M_2)` returns `Mapping_C` (1 property)
    ///
    /// The unfiltered `.nth(2)` form would return `Mapping_A` for `body(M_2)`
    /// (the 3rd top-level child is `mapping_0` because `prefix` + `source_def`
    /// occupy positions 0 and 1), so the wrong test would see 3 properties
    /// where 1 is expected. That's the silent corruption the filter-before-
    /// nth ordering prevents.
    #[test]
    fn body_filters_to_mapping_kind_before_indexing() {
        let src = "\
prefix ex: <https://example.org/>

users := io.csv(\"x.csv\")

Mapping_A : ex:Shape from users
    ex:a = .a
    ex:b = .b
    ex:c = .c

Mapping_B : ex:Shape from users
    ex:a = .a
    ex:b = .b

Mapping_C : ex:Shape from users
    ex:c = .c
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, src.to_string(), "indexing.fossil".to_string());
        let dm = def_map(&db, file);
        let mappings = dm.mappings(&db);
        assert_eq!(
            mappings.len(),
            3,
            "expected 3 MappingLocs for 3 MAPPINGs (got {})",
            mappings.len()
        );

        let b_a = body(&db, mappings[0]);
        let b_b = body(&db, mappings[1]);
        let b_c = body(&db, mappings[2]);
        assert_eq!(
            b_a.properties(&db).len(),
            3,
            "Mapping_A must have 3 properties"
        );
        assert_eq!(
            b_b.properties(&db).len(),
            2,
            "Mapping_B must have 2 properties"
        );
        assert_eq!(
            b_c.properties(&db).len(),
            1,
            "Mapping_C must have 1 property (NOT 3 — that would be the \
             unfiltered .nth(2) bug returning Mapping_A's body)"
        );
    }

    #[test]
    fn body_is_memoised_per_mapping() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
        let dm = def_map(&db, file);
        let m = dm.mappings(&db)[0];
        let a = body(&db, m);
        let b = body(&db, m);
        assert_eq!(a, b);
    }
}
