//! Phase 1 placeholder for the stable item tree.
//!
//! Phase 2 (CORE-02) makes [`ItemTree`] stable across body edits per the
//! rust-analyzer pattern: editing the right-hand side of a property MUST NOT
//! invalidate downstream queries that only depend on the structural skeleton
//! (mapping names, shape IRIs, source bindings). A Salsa invalidation
//! regression test is part of CORE-02's exit criteria.
//!
//! Phase 1 stores only the count of top-level items so the query exists and
//! the public signature is locked for Phase 2-9 consumers.

use fossil_base::SourceFile;

#[salsa::tracked(debug)]
pub struct ItemTree<'db> {
    /// Phase 1: count of top-level items only. Phase 2 (CORE-02) stores the
    /// structural skeleton: a `Vec<ItemHeader>` whose hash is invariant under
    /// body-only edits.
    pub item_count: usize,
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn item_tree<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> ItemTree<'db> {
    let cst = fossil_syntax::parse(db, file);
    let count = cst.root(db).syntax().children().count();
    ItemTree::new(db, count)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

    #[test]
    fn item_tree_counts_three_top_level_items_for_hello_fossil() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        let it = item_tree(&db, file);
        // Phase 1 placeholder counts ALL top-level CST children including
        // any trivia-wrapping; with the current parser shape there are
        // exactly 3 named items: PREFIX_DECL, SOURCE_DEF, MAPPING.
        assert_eq!(it.item_count(&db), 3);
    }
}
