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
//! [`body`] resolves the target mapping by `cst.root(db).syntax().children()
//! .filter(|n| n.kind() == SyntaxKind::MAPPING).nth(idx)`. The `.filter(...)`
//! MUST come BEFORE `.nth(idx)`: without it, `PREFIX_DECL` / `SOURCE_DEF` /
//! `IMPORT` top-level children inflate the count and `.nth(idx)` returns the
//! wrong node. The regression test
//! [`tests::body_filters_to_mapping_kind_before_indexing`] enforces this.

use crate::def_map::{MappingLoc, def_map};
use crate::lower::{HirProperty, lower_property_public};
use fossil_syntax::SyntaxKind;

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

/// Per-mapping body query. Keyed by the interned [`MappingLoc`]; depends on
/// the file's CST (via [`fossil_syntax::parse`]) and on the file's prefix
/// table (via [`def_map`]). Body content is read from EXACTLY ONE mapping —
/// editing a sibling mapping's body does NOT invalidate `body(M_k)`.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn body<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> HirBody<'db> {
    let file = mapping.file(db);
    let idx = mapping.index(db);
    let cst = fossil_syntax::parse(db, file);
    let dm = def_map(db, file);
    let prefixes = dm.prefixes(db);

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

    let mut properties: Vec<HirProperty> = Vec::new();
    let mut expr_count: u32 = 0;
    if let Some(node) = mapping_node
        && let Some(body_node) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)
    {
        for prop_node in body_node
            .children()
            .filter(|c| c.kind() == SyntaxKind::PROPERTY)
        {
            if let Some(prop) = lower_property_public(&prop_node, prefixes) {
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
        let dm = def_map(&db, file);
        let m = dm.mappings(&db)[0];
        let a = body(&db, m);
        let b = body(&db, m);
        assert_eq!(a, b);
    }
}
