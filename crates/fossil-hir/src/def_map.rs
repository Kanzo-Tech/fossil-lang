//! [`DefMap`] — per-file name resolution table.
//!
//! Holds the prefix table (`prefix ex: <...>`), the source bindings
//! (`users := io.csv("...")`), and the mapping list (one entry per `Name :
//! Shape from source` block). Populated by the [`def_map`] Salsa query, which
//! consumes the CST from [`fossil_syntax::parse`].
//!
//! Per RESEARCH.md §"Architecture Patterns" Pattern 2 + 3, mapping and source
//! locations are interned with a `'db` lifetime so downstream queries can be
//! keyed per-item (rust-analyzer's per-item Salsa fan-out). Even though the
//! Phase 1 example has only one mapping, the SHAPE of the query graph matters
//! for Phase 2's stable item tree (CORE-02) and Phase 6's LSP perf benchmark.
//!
//! # Storage choice
//!
//! Prefix and source tables are stored as insertion-ordered `Vec<(K, V)>`
//! pairs rather than the originally-planned `IndexMap<K, V>`. The workspace
//! `indexmap = "2"` pin does not implement [`std::hash::Hash`] on `IndexMap`,
//! and Salsa tracked-struct fields require Hash for memoisation. Insertion
//! order is preserved either way; callers that need O(1) lookup can build a
//! `HashMap` at the call site (or use the [`DefMap::lookup_prefix`] /
//! [`DefMap::lookup_source`] helpers added below).

use fossil_base::SourceFile;
use smol_str::SmolStr;

#[salsa::interned(debug)]
pub struct MappingLoc<'db> {
    pub file: SourceFile,
    pub index: usize,
}

#[salsa::interned(debug)]
pub struct SourceLoc<'db> {
    pub file: SourceFile,
    pub index: usize,
}

/// One entry in the prefix table (`prefix ex: <https://example.org/>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PrefixEntry {
    pub name: SmolStr,
    pub iri: SmolStr,
}

/// One entry in the source-binding table (`users := io.csv(...)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceEntry<'db> {
    pub name: SmolStr,
    pub loc: SourceLoc<'db>,
}

#[salsa::tracked(debug)]
pub struct DefMap<'db> {
    #[returns(ref)]
    pub prefixes: Vec<PrefixEntry>,
    #[returns(ref)]
    pub sources: Vec<SourceEntry<'db>>,
    #[returns(ref)]
    pub mappings: Vec<MappingLoc<'db>>,
}

impl<'db> DefMap<'db> {
    /// Look up the expanded IRI for a prefix name. O(n) over the prefix list;
    /// fine for Phase 1 (single-digit prefix counts in real `.fossil` files).
    /// Phase 2 may swap the storage for an interned `IndexMap` once the Salsa
    /// `Hash` constraint is resolved upstream.
    #[must_use]
    pub fn lookup_prefix(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SmolStr> {
        self.prefixes(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.iri.clone())
    }

    /// Look up the [`SourceLoc`] bound to a source name (e.g. `users`).
    #[must_use]
    pub fn lookup_source(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SourceLoc<'db>> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.loc)
    }
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn def_map<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> DefMap<'db> {
    let cst = fossil_syntax::parse(db, file);
    let mut prefixes: Vec<PrefixEntry> = Vec::new();
    let mut sources: Vec<SourceEntry<'db>> = Vec::new();
    let mut mappings: Vec<MappingLoc<'db>> = Vec::new();

    // Per-kind dense indices. Plan 02-03 §"def_map.rs" requires mapping
    // indexing among MAPPING-kind children only — Plan 02-04 keys its
    // `body(mapping)` Salsa query on `MappingLoc`, and the Salsa-invalidation
    // story is cleanest when adding an unrelated `prefix` or `source_def` to
    // the file does NOT shift every downstream mapping's `index` (and hence
    // its interned `MappingLoc`). The mir lowering (`fossil-mir::lower`) used
    // to recover the dense index from `def_map.mappings()`; with dense
    // indexing the recovery step becomes trivial (`loc.index(db)` is the
    // dense index directly).
    //
    // SOURCE_DEF indexing follows the same per-kind dense scheme for
    // symmetry; no current downstream consumer depends on the all-children
    // index space for SOURCE_DEF either.
    let mut mapping_idx = 0usize;
    let mut source_idx = 0usize;
    for item in cst.root(db).syntax().children() {
        use fossil_syntax::SyntaxKind;
        match item.kind() {
            SyntaxKind::PREFIX_DECL => {
                if let Some((name, iri)) = parse_prefix_decl_node(&item) {
                    prefixes.push(PrefixEntry { name, iri });
                }
            }
            SyntaxKind::SOURCE_DEF => {
                if let Some(name) = parse_source_name(&item) {
                    sources.push(SourceEntry {
                        name,
                        loc: SourceLoc::new(db, file, source_idx),
                    });
                    source_idx += 1;
                }
            }
            SyntaxKind::MAPPING => {
                mappings.push(MappingLoc::new(db, file, mapping_idx));
                mapping_idx += 1;
            }
            _ => {}
        }
    }

    DefMap::new(db, prefixes, sources, mappings)
}

/// Extract `(prefix-name, expanded-IRI)` from a `PREFIX_DECL` node.
///
/// Mirrors the AST view's [`fossil_syntax::ast::PrefixDecl`] accessor logic
/// but operates directly on the `SyntaxNode` to keep the lowering self-
/// contained (no AST cast indirection on the Salsa-tracked path).
fn parse_prefix_decl_node(node: &fossil_syntax::SyntaxNode) -> Option<(SmolStr, SmolStr)> {
    use fossil_syntax::SyntaxKind;
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .collect();
    let name = toks.iter().find(|t| t.kind() == SyntaxKind::IDENT)?;
    let abs_iri = toks.iter().find(|t| t.kind() == SyntaxKind::ABS_IRI)?;
    let iri_text = abs_iri.text().trim_start_matches('<').trim_end_matches('>');
    Some((SmolStr::from(name.text()), SmolStr::from(iri_text)))
}

/// Extract the bound name from a `SOURCE_DEF` node (the `users` in
/// `users := io.csv("...")`). The first IDENT child token is the binding name;
/// IDENTs nested inside `CALL_EXPR` belong to the callee.
fn parse_source_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    let ident = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    Some(SmolStr::from(ident.text()))
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

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    #[test]
    fn def_map_for_hello_fossil_has_one_prefix_one_source_one_mapping() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        assert_eq!(dm.prefixes(&db).len(), 1);
        assert_eq!(
            dm.lookup_prefix(&db, "ex").as_deref(),
            Some("https://example.org/")
        );
        assert_eq!(dm.sources(&db).len(), 1);
        assert!(dm.lookup_source(&db, "users").is_some());
        assert_eq!(dm.mappings(&db).len(), 1);
    }

    #[test]
    fn def_map_source_loc_is_interned_per_file() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let src = dm.lookup_source(&db, "users").unwrap();
        // Plan 02-03: per-kind dense indexing. `users` is the only
        // SOURCE_DEF in hello.fossil, so its dense index is 0.
        assert_eq!(src.index(&db), 0);
        assert_eq!(src.file(&db), file);
    }

    #[test]
    fn def_map_mapping_loc_indexes_mapping_position() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let mappings = dm.mappings(&db);
        assert_eq!(mappings.len(), 1);
        // Plan 02-03: per-kind dense indexing. `User` is the only MAPPING
        // in hello.fossil, so its dense index is 0 (was 2 under the
        // legacy all-children index space).
        assert_eq!(mappings[0].index(&db), 0);
        assert_eq!(mappings[0].file(&db), file);
    }

    #[test]
    fn def_map_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let a = def_map(&db, file);
        let b = def_map(&db, file);
        // Salsa-tracked structs compare by interned id; identical inputs
        // must yield the same handle (memoisation hit).
        assert_eq!(a, b);
    }
}
