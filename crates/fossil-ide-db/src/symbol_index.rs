//! [`SymbolIndex`] — the per-file table of named definitions.
//!
//! Built by walking the top-level items of [`fossil_syntax::parse`]'s CST and
//! recording, for each definition, its `{ name, kind, range }`. The range is a
//! byte range (`Range<u32>`) into the file text, so a downstream goto-def
//! consumer (plan 06-06) can map a hit back to an LSP location.
//!
//! # Why a fresh CST walk and not `fossil-hir::def_map`?
//!
//! `def_map` interns `MappingLoc`/`SourceLoc` (per-item Salsa keys) and does
//! NOT carry byte ranges or function/shape names — it is the type-checker's
//! signature table. The IDE search layer needs ranges (for goto-def
//! highlighting) and the full `{prefix, mapping, function, shape}` symbol set
//! (for the outline + completion), so it walks the CST directly. Crucially this
//! keeps the symbol index FILE-keyed: it is rebuilt whole on any edit (one
//! re-run, Research Pitfall #3 — NO per-mapping Salsa key is added here, so the
//! per-mapping `body()` fan-out stays at 1).

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_syntax::ast::{Mapping, PrefixDecl};
use fossil_syntax::{SyntaxKind, SyntaxNode};
use smol_str::SmolStr;

/// The classification of a [`SymbolEntry`].
///
/// Mirrors the LSP `SymbolKind` axes the outline maps onto later (plan 06-06):
/// `Prefix → Namespace`, `Mapping → Class/Struct`, `Function → Function`,
/// `Shape → Interface`. Kept as a Fossil-native enum here so `fossil-ide-db`
/// does not need an `lsp-types` dependency for the index itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// A `prefix xx: <iri>` declaration.
    Prefix,
    /// A mapping header name (`User` in `User : ex:Person from users`).
    Mapping,
    /// A top-level function definition (`f := …`, exported or not).
    Function,
    /// A shape reference used in a mapping header (`ex:Person`).
    Shape,
}

/// One named definition recorded in a [`SymbolIndex`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolEntry {
    /// The symbol's surface name (`ex` for a prefix, `User` for a mapping,
    /// `ex:Person` for a shape ref, `f` for a function).
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
            match item.kind() {
                SyntaxKind::PREFIX_DECL => {
                    if let Some(name) = PrefixDecl::cast(item.clone()).and_then(|d| d.name()) {
                        entries.push(SymbolEntry {
                            name,
                            kind: SymbolKind::Prefix,
                            range: node_range(&item),
                        });
                    }
                }
                SyntaxKind::MAPPING => {
                    if let Some(header) = Mapping::cast(item.clone()).and_then(|m| m.header()) {
                        if let Some(name) = header.name() {
                            entries.push(SymbolEntry {
                                name,
                                kind: SymbolKind::Mapping,
                                range: node_range(&item),
                            });
                        }
                        // A mapping's header carries a shape ref (`ex:Person`);
                        // record it so goto-def on the shape resolves to its
                        // declaring mapping site.
                        if let Some(iri) = header.shape_expr().and_then(|s| s.primary_iri()) {
                            let s = iri.syntax();
                            entries.push(SymbolEntry {
                                name: SmolStr::from(s.text().to_string()),
                                kind: SymbolKind::Shape,
                                range: node_range(s),
                            });
                        }
                    }
                }
                // Functions: both bare `f := …` (DEFINITION) and exported
                // `@export f := …` (EXPORTED_DEFINITION). The exported wrapper
                // contains the DEFINITION; reach through to its name.
                SyntaxKind::DEFINITION => {
                    push_function(&mut entries, &item);
                }
                SyntaxKind::EXPORTED_DEFINITION => {
                    if let Some(def) = item.children().find(|c| c.kind() == SyntaxKind::DEFINITION)
                    {
                        // Range spans the whole exported item so goto-def lands
                        // on the `@export` line.
                        push_function_with_range(&mut entries, &def, node_range(&item));
                    }
                }
                _ => {}
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

/// Record a function definition from a `DEFINITION` node (range = the node).
fn push_function(entries: &mut Vec<SymbolEntry>, node: &SyntaxNode) {
    push_function_with_range(entries, node, node_range(node));
}

/// Record a function definition from a `DEFINITION` node with an explicit range
/// (used for the `EXPORTED_DEFINITION` wrapper case).
fn push_function_with_range(entries: &mut Vec<SymbolEntry>, def: &SyntaxNode, range: Range<u32>) {
    if let Some(name) = fossil_syntax::ast::Definition::cast(def.clone()).and_then(|d| d.name()) {
        entries.push(SymbolEntry {
            name,
            kind: SymbolKind::Function,
            range,
        });
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        SymbolIndex::build(&db, file)
    }

    const TWO_MAPPINGS: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")

User : ex:Person from users
    ex:name = .name

Org : ex:Organization from users
    ex:title = .title
";

    #[test]
    fn enumerates_prefix_mappings_and_shapes() {
        let idx = index_of(TWO_MAPPINGS);
        // 1 prefix.
        assert_eq!(idx.of_kind(SymbolKind::Prefix).count(), 1);
        // 2 mappings.
        let mappings: Vec<_> = idx.of_kind(SymbolKind::Mapping).collect();
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].name.as_str(), "User");
        assert_eq!(mappings[1].name.as_str(), "Org");
        // 2 shape refs.
        let shapes: Vec<_> = idx.of_kind(SymbolKind::Shape).collect();
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].name.as_str(), "ex:Person");
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
        let entry = idx.lookup("Org").expect("Org is defined");
        assert_eq!(entry.kind, SymbolKind::Mapping);
    }

    #[test]
    fn enumerates_function_definitions() {
        // A top-level function definition is an EXPORTED_DEFINITION
        // (`@export f := …`); a bare `IDENT := …` parses as a SOURCE_DEF per
        // the parser's Phase-1/2 split (parser/items.rs).
        let src = "@export ident := .x\n";
        let idx = index_of(src);
        let funcs: Vec<_> = idx.of_kind(SymbolKind::Function).collect();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name.as_str(), "ident");
    }
}
