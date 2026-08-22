//! [`SymbolIndex`] — the per-file table of named definitions.
//!
//! Built by walking the top-level items of [`fossil_syntax::parse`]'s CST and
//! recording, for each definition, its `{ name, kind, range }`. The range is a
//! byte range (`Range<u32>`) into the file text, so a downstream goto-def
//! consumer can map a hit back to an LSP location.
//!
//! # Why a fresh CST walk and not `fossil-hir::def_map`?
//!
//! `def_map` interns `MappingLoc`/`SourceLoc` (per-item Salsa keys) and does
//! NOT carry byte ranges or function/shape names — it is the type-checker's
//! signature table. The IDE search layer needs ranges (for goto-def
//! highlighting) and the full `{prefix, mapping, function, shape}` symbol set
//! (for the outline + completion), so it walks the CST directly. Crucially this
//! keeps the symbol index FILE-keyed: it is rebuilt whole on any edit (one
//! re-run — NO per-mapping Salsa key is added here, so the
//! per-mapping `body()` fan-out stays at 1).

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_syntax::ast::Mapping;
use fossil_syntax::{SyntaxKind, SyntaxNode};
use smol_str::SmolStr;

/// The classification of a [`SymbolEntry`].
///
/// Mirrors the LSP `SymbolKind` axes the outline maps onto:
/// `Mapping → Class/Struct`, `Shape → Interface`. Kept
/// as a Fossil-native enum so the index itself owes nothing to `lsp-types` —
/// [`crate::outline`] is where the translation lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    // A `Prefix` variant mapped to the LSP `Namespace` kind and indexed
    // `prefix xx: <iri>`. The declaration is gone, and with it the only symbol
    // this crate ever indexed that was not a mapping or a shape.
    /// A mapping header name (`Users` in `Users : Person from Adults`).
    Mapping,
    /// The shape name a mapping header targets (`Person`).
    Shape,
}

/// One named definition recorded in a [`SymbolIndex`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolEntry {
    /// The symbol's surface name (`Users` for a mapping, `Person` for the
    /// shape it targets).
    pub name: SmolStr,
    /// What kind of definition this is.
    pub kind: SymbolKind,
    /// Byte range of the defining token/node into the file text.
    pub range: Range<u32>,
}

/// Per-file symbol table. See the module docs for why this is a plain struct
/// built by a CST walk rather than a Salsa-tracked query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymbolIndex {
    entries: Vec<SymbolEntry>,
}

impl SymbolIndex {
    /// Build the index for `file` by walking the CST top-level items.
    #[must_use]
    pub fn build(db: &dyn fossil_base::Db, file: SourceFile) -> Self {
        let cst = fossil_syntax::parse(db, file);
        let root = cst.root(db).syntax();
        Self::from_root(&root)
    }

    /// Build the index directly from a parsed root node. Split out from
    /// [`Self::build`] so unit tests can drive it off a `parse` result without
    /// re-deriving the `Db`.
    #[must_use]
    pub fn from_root(root: &SyntaxNode) -> Self {
        let mut entries = Vec::new();
        for item in root.children() {
            if item.kind() == SyntaxKind::MAPPING
                && let Some(header) = Mapping::cast(item.clone()).and_then(|m| m.header())
            {
                if let Some(name) = header.name() {
                    entries.push(SymbolEntry {
                        name,
                        kind: SymbolKind::Mapping,
                        range: node_range(&item),
                    });
                }
                // A mapping's header carries a shape NAME (`Person`);
                // record it so goto-def on the shape resolves to the
                // `type { … } := …` binding that introduced it.
                if let Some(shape) = header.shape_expr()
                    && let Some(name) = shape.name()
                {
                    entries.push(SymbolEntry {
                        name,
                        kind: SymbolKind::Shape,
                        range: node_range(shape.syntax()),
                    });
                }
            }
        }
        Self { entries }
    }

    /// Look up the FIRST entry with the given name (any kind).
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<&SymbolEntry> {
        self.entries.iter().find(|e| e.name.as_str() == name)
    }

    /// All entries (insertion order = source order).
    #[must_use]
    pub fn entries(&self) -> &[SymbolEntry] {
        &self.entries
    }

    /// All entries of a given kind.
    pub fn of_kind(&self, kind: SymbolKind) -> impl Iterator<Item = &SymbolEntry> {
        self.entries.iter().filter(move |e| e.kind == kind)
    }
}

/// Byte range of a node as `Range<u32>` (rowan offsets are `u32`).
fn node_range(node: &SyntaxNode) -> Range<u32> {
    let r = node.text_range();
    u32::from(r.start())..u32::from(r.end())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn index_of(src: &str) -> SymbolIndex {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        SymbolIndex::build(&db, file)
    }

    const TWO_MAPPINGS: &str = "\
type { Person, Organization } := io.shex(\"vocab.shex\")

User := io.csv(\"users.csv\")

Users : Person from User
    name = User.name

Orgs : Organization from User
    title = User.title
";

    #[test]
    fn enumerates_mappings_and_shapes() {
        let idx = index_of(TWO_MAPPINGS);
        // 2 mappings. There is no third kind: the `prefix` line this fixture
        // opened with, and the `SymbolKind::Prefix` entry it produced, are both
        // gone: a program declares no vocabulary.
        let mappings: Vec<_> = idx.of_kind(SymbolKind::Mapping).collect();
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].name.as_str(), "Users");
        assert_eq!(mappings[1].name.as_str(), "Orgs");
        // 2 shape names, bare — `Person`, not `ex:Person`.
        let shapes: Vec<_> = idx.of_kind(SymbolKind::Shape).collect();
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].name.as_str(), "Person");
    }

    #[test]
    fn ranges_are_nonempty_and_in_bounds() {
        let idx = index_of(TWO_MAPPINGS);
        let len = u32::try_from(TWO_MAPPINGS.len()).unwrap();
        for e in idx.entries() {
            assert!(e.range.start < e.range.end, "empty range for {}", e.name);
            assert!(e.range.end <= len, "range OOB for {}", e.name);
        }
    }

    #[test]
    fn lookup_finds_a_mapping() {
        let idx = index_of(TWO_MAPPINGS);
        let entry = idx.lookup("Orgs").expect("Orgs is defined");
        assert_eq!(entry.kind, SymbolKind::Mapping);
    }
}
