//! [`PrefixIndex`] — prefix → IRI resolution for a file.
//!
//! Collects the `prefix xx: <iri>` declarations recorded in a file's
//! [`SymbolIndex`] (re-walking the CST for the IRI text, which the symbol entry
//! does not carry) and layers the well-known RDF prefixes (`rdf`, `rdfs`,
//! `xsd`, `owl`) underneath as auto-importable constants.
//!
//! Resolution order: a file-declared prefix WINS over a well-known one (the
//! author may legitimately re-bind `rdf:` to a different IRI). Consumers:
//! completion (plan 06-06) offers the well-known set as auto-import items; the
//! unknown-prefix quick-fix (Wave 4) reads the canonical IRIs.

use fossil_base::SourceFile;
use fossil_syntax::ast::PrefixDecl;
use fossil_syntax::{SyntaxKind, SyntaxNode};
use smol_str::SmolStr;

/// The well-known RDF prefixes and their canonical IRIs, offered as
/// auto-importable completions and used by the unknown-prefix quick-fix.
pub const WELL_KNOWN_PREFIXES: &[(&str, &str)] = &[
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
];

/// One declared prefix binding (`prefix ex: <https://example.org/>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrefixBinding {
    /// The prefix name (`ex`).
    pub prefix: SmolStr,
    /// The expanded IRI, `<>` stripped.
    pub iri: SmolStr,
}

/// Per-file prefix table with a well-known fallback layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrefixIndex {
    declared: Vec<PrefixBinding>,
}

impl PrefixIndex {
    /// Build the index for `file` by walking its `PREFIX_DECL` items.
    #[must_use]
    pub fn build(db: &dyn fossil_base::Db, file: SourceFile) -> Self {
        let cst = fossil_syntax::parse(db, file);
        Self::from_root(&cst.root(db).syntax())
    }

    /// Build directly from a parsed root node.
    #[must_use]
    pub fn from_root(root: &SyntaxNode) -> Self {
        let mut declared = Vec::new();
        for item in root.children() {
            if item.kind() != SyntaxKind::PREFIX_DECL {
                continue;
            }
            if let Some(decl) = PrefixDecl::cast(item)
                && let (Some(prefix), Some(iri)) = (decl.name(), decl.iri())
            {
                declared.push(PrefixBinding { prefix, iri });
            }
        }
        Self { declared }
    }

    /// Resolve a prefix name to its IRI. A file-declared binding takes
    /// precedence over a well-known one; `None` if neither matches.
    #[must_use]
    pub fn resolve(&self, prefix: &str) -> Option<&str> {
        if let Some(b) = self.declared.iter().find(|b| b.prefix.as_str() == prefix) {
            return Some(b.iri.as_str());
        }
        WELL_KNOWN_PREFIXES
            .iter()
            .find(|(p, _)| *p == prefix)
            .map(|(_, iri)| *iri)
    }

    /// The prefixes declared in THIS file (excludes the well-known fallback).
    #[must_use]
    pub fn declared(&self) -> &[PrefixBinding] {
        &self.declared
    }

    /// Whether a prefix is declared in this file (NOT counting well-known).
    /// The auto-import quick-fix uses this to decide whether to insert a
    /// `prefix xx: <...>` line.
    #[must_use]
    pub fn is_declared(&self, prefix: &str) -> bool {
        self.declared.iter().any(|b| b.prefix.as_str() == prefix)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn index_of(src: &str) -> PrefixIndex {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        PrefixIndex::build(&db, file)
    }

    #[test]
    fn resolves_a_declared_prefix() {
        let idx = index_of("prefix ex: <https://example.org/>\n");
        assert_eq!(idx.resolve("ex"), Some("https://example.org/"));
        assert!(idx.is_declared("ex"));
    }

    #[test]
    fn resolves_a_well_known_prefix_without_declaration() {
        let idx = index_of("prefix ex: <https://example.org/>\n");
        assert_eq!(
            idx.resolve("xsd"),
            Some("http://www.w3.org/2001/XMLSchema#")
        );
        // xsd is NOT declared in the file — only resolved via the fallback.
        assert!(!idx.is_declared("xsd"));
    }

    #[test]
    fn declared_prefix_overrides_well_known() {
        let idx = index_of("prefix rdf: <https://shadowed.example/rdf#>\n");
        assert_eq!(idx.resolve("rdf"), Some("https://shadowed.example/rdf#"));
    }

    #[test]
    fn unknown_prefix_is_none() {
        let idx = index_of("prefix ex: <https://example.org/>\n");
        assert_eq!(idx.resolve("nope"), None);
    }
}
