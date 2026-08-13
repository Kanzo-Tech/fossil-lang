//! `AstIdMap` — stable typed identity for AST nodes across body-only edits.
//!
//! Pattern: rust-analyzer's `hir-def::AstIdMap`.
//!
//! Why: [`crate::item_tree::ItemHeader`] stores `FileAstId<MappingNode>` (a
//! typed index into this map) instead of a `SyntaxNodePtr` or a raw byte
//! offset. The index is assigned during a DFS top-level scan in insertion
//! order, so adding/editing one mapping does NOT renumber the others — the
//! previous indices remain valid pointers to the same node identity.
//!
//! `'db` lifetime per Salsa 0.26 — RESEARCH.md §Q7. Reference: rust-analyzer
//! PR #19495 ("Start infesting ide crates with `'db` lifetime").

use fossil_base::SourceFile;
use fossil_syntax::SyntaxKind;
use std::marker::PhantomData;

/// Typed index into [`AstIdMap`].
///
/// E.g. `FileAstId<MappingNode>` for the N-th MAPPING in the file (DFS
/// top-level scan order, stable across body edits because rowan green-node
/// reuse means unchanged subtrees keep the same identity).
///
/// The `PhantomData<N>` is purely a type-level marker — at runtime this is a
/// `u32`. Two different `FileAstId<X>` and `FileAstId<Y>` cannot be mixed
/// because the marker is part of the type.
#[derive(Debug)]
pub struct FileAstId<N> {
    raw: u32,
    _phantom: PhantomData<fn() -> N>,
}

// Manual impls of `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash` so that they do
// NOT require `N: Trait`. The `PhantomData<fn() -> N>` makes derives behave
// well in principle, but being explicit avoids surprising bounds when a
// downstream caller uses a non-`Clone` marker type.
impl<N> Clone for FileAstId<N> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<N> Copy for FileAstId<N> {}
impl<N> PartialEq for FileAstId<N> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<N> Eq for FileAstId<N> {}
impl<N> std::hash::Hash for FileAstId<N> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

// SAFETY: third-party-trait integration boundary — the workspace denies
// `unsafe_code` rather than forbidding it, so a boundary like this one opts in
// with an explicit `allow` and this justification. `FileAstId`
// is `Copy + Eq`, so the trivial-replace pattern is sound: the new value
// either equals the old (no change) or replaces it bit-for-bit (self-
// contained, no nested invariants). The `PhantomData` carries no runtime
// state. No safe alternative exists because `salsa::Update` requires
// `unsafe impl` even for trivially-safe bodies.
#[allow(unsafe_code)]
unsafe impl<N: 'static> salsa::Update for FileAstId<N> {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: caller guarantees `old_pointer` is a valid, aligned pointer
        // to an initialised `FileAstId<N>` owned by Salsa storage.
        let old = unsafe { &mut *old_pointer };
        if *old == new_value {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

impl<N> FileAstId<N> {
    /// Raw `u32` index — exposed for diagnostics and tests; not for normal
    /// query plumbing (use the typed `FileAstId<N>` instead).
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.raw
    }

    pub(crate) fn new(raw: u32) -> Self {
        Self {
            raw,
            _phantom: PhantomData,
        }
    }
}

/// Marker type — used as `FileAstId<MappingNode>` for indices that point at a
/// top-level `MAPPING` CST node.
#[derive(Debug)]
pub struct MappingNode;
/// Marker type — `FileAstId<SourceDefNode>`.
#[derive(Debug)]
pub struct SourceDefNode;
// There was a `PrefixDeclNode` marker here, and an `ImportNode` before it.
// `use` left the grammar with the module system there never was, and `prefix`
// with the CURIE — `use` and `prefix` are ordinary identifiers now — so neither
// has a node to point a `FileAstId` at.

/// Per-file stable map: records the DFS top-level insertion order of every
/// signature-carrying CST node.
///
/// The map is `#[salsa::tracked]` so it is memoised — but its input is the
/// top-level *structure* of the CST (the sequence of top-level node kinds),
/// not body content. When a mapping body changes, the green-node hash for
/// the MAPPING node DOES change, but the AstIdMap's input — the sequence of
/// top-level kinds — does not, so the `FileAstId` indices for unchanged
/// mappings stay stable.
#[salsa::tracked(debug)]
pub struct AstIdMap<'db> {
    /// Per-entry record: kind tag + DFS top-level local index. The vector is
    /// indexed by the `FileAstId<N>` raw value, so reverse lookup ("the
    /// `FileAstId::raw == 3` entry is a MAPPING at top-level child #3") is a
    /// trivial vector index.
    #[returns(ref)]
    pub entries: Vec<AstIdEntry>,
}

/// One record in [`AstIdMap::entries`].
///
/// `kind` discriminates which marker type the `FileAstId` belongs to;
/// `local_index` is the position among the top-level CST children (NOT the
/// per-kind dense index — that's `MappingLoc::index` / `SourceLoc::index`
/// over in [`crate::def_map`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct AstIdEntry {
    pub kind: SyntaxKind,
    /// Position of this item among siblings in DFS top-level scan order.
    pub local_index: u32,
}

/// Build the per-file `AstIdMap` by walking top-level CST children once. Body
/// content is NOT inspected — that's the invalidation-barrier guarantee
/// (RESEARCH.md §Q3).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn ast_id_map<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> AstIdMap<'db> {
    let cst = fossil_syntax::parse(db, file);
    let mut entries: Vec<AstIdEntry> = Vec::new();
    for (idx, child) in cst.root(db).syntax().children().enumerate() {
        // We DO NOT inspect child.children() or child.text() here. Body
        // content does not contribute to AstIdMap's input.
        match child.kind() {
            SyntaxKind::MAPPING | SyntaxKind::SOURCE_DEF => {
                entries.push(AstIdEntry {
                    kind: child.kind(),
                    local_index: u32::try_from(idx).expect("file with > u32::MAX top-level items"),
                });
            }
            _ => {} // skip trivia, errors at top level
        }
    }
    AstIdMap::new(db, entries)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    #[allow(clippy::literal_string_with_formatting_args)] // Fossil-source fixture, not a format string
    fn ast_id_map_assigns_stable_indices_in_dfs_order() {
        let src = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"x.csv\")

Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "test.fossil".to_string());
        let m = ast_id_map(&db, file);
        let entries = m.entries(&db);
        // TWO, not three. The `type { … } := …` binding above them carries no
        // `FileAstId` — `TYPE_DEF` is not one of the signature-carrying kinds —
        // where the `prefix` line this fixture used to open with was.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, SyntaxKind::SOURCE_DEF);
        assert_eq!(entries[1].kind, SyntaxKind::MAPPING);
    }

    #[test]
    fn ast_id_map_memoises_per_file() {
        let src = "User := io.csv(\"u.csv\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "m.fossil".to_string());
        let a = ast_id_map(&db, file);
        let b = ast_id_map(&db, file);
        assert_eq!(a, b);
    }
}
