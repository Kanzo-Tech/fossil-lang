//! [`WorkspaceIndex`] — cross-file symbol aggregation.
//!
//! Aggregates the per-file [`SymbolIndex`]es of a SET of [`SourceFile`]s into a
//! single resolver, so a symbol declared in file A resolves when referenced
//! from file B. This implements the **open-files-as-workspace** model
//! (ADR-0023): the file set is whatever the host holds open — the LSP's
//! `LspState.files` map or the playground's multi-panel set — NOT a filesystem
//! scan of a workspace root (deferred to v2; conflicts with the WASM
//! `VirtualFS`).
//!
//! Like [`SymbolIndex`] this is a plain struct (no Salsa query of its own), so
//! it adds zero tracked queries and keeps the per-mapping `body()` fan-out at 1
//! (Research Pitfall #3). It is rebuilt from the open-file set on demand; the
//! per-file `SymbolIndex::build` underneath is itself cheap (one CST walk).

use fossil_base::SourceFile;

use crate::prefix_index::PrefixIndex;
use crate::symbol_index::{SymbolEntry, SymbolIndex};

/// Cross-file symbol resolver over the open-file set.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceIndex {
    files: Vec<(SourceFile, SymbolIndex, PrefixIndex)>,
}

impl WorkspaceIndex {
    /// Build the workspace index by indexing every file in `files`.
    ///
    /// The order of `files` is preserved; [`Self::resolve`] returns matches in
    /// that order so a deterministic "first declaration wins" policy can be
    /// applied by the caller if needed.
    #[must_use]
    pub fn build(db: &dyn fossil_base::Db, files: &[SourceFile]) -> Self {
        let files = files
            .iter()
            .map(|&file| {
                let symbols = SymbolIndex::build(db, file);
                let prefixes = PrefixIndex::build(db, file);
                (file, symbols, prefixes)
            })
            .collect();
        Self { files }
    }

    /// Resolve a symbol name to ALL matching definitions across the open files.
    ///
    /// Returns `(file, entry)` pairs so the caller (goto-def, plan 06-06) can
    /// build an LSP location from the owning file + the entry's byte range. A
    /// name declared in file A and referenced in file B resolves via the union.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Vec<(SourceFile, SymbolEntry)> {
        let mut hits = Vec::new();
        for (file, symbols, _) in &self.files {
            for entry in symbols.entries() {
                if entry.name.as_str() == name {
                    hits.push((*file, entry.clone()));
                }
            }
        }
        hits
    }

    /// Resolve a prefix name to ALL `(file, iri)` declarations across the open
    /// files. The well-known fallback is intentionally NOT folded in here — a
    /// well-known prefix has no declaring file/site, so completion handles it
    /// separately via [`crate::prefix_index::WELL_KNOWN_PREFIXES`].
    #[must_use]
    pub fn resolve_prefix(&self, prefix: &str) -> Vec<(SourceFile, &str)> {
        let mut hits = Vec::new();
        for (file, _, prefixes) in &self.files {
            for binding in prefixes.declared() {
                if binding.prefix.as_str() == prefix {
                    hits.push((*file, binding.iri.as_str()));
                }
            }
        }
        hits
    }

    /// The per-file [`SymbolIndex`] for one open file, if present.
    #[must_use]
    pub fn symbols_of(&self, file: SourceFile) -> Option<&SymbolIndex> {
        self.files
            .iter()
            .find(|(f, _, _)| *f == file)
            .map(|(_, s, _)| s)
    }

    /// The set of files this index spans.
    pub fn files(&self) -> impl Iterator<Item = SourceFile> + '_ {
        self.files.iter().map(|(f, _, _)| *f)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::symbol_index::SymbolKind;
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        fossil_base::FossilDb::new(system)
    }

    #[test]
    fn resolves_a_mapping_across_two_files() {
        let db = db();
        let a = fossil_base::SourceFile::new(
            &db,
            "prefix ex: <https://example.org/>\n\nUser : ex:Person from users\n    ex:name = .name\n"
                .to_string(),
            "a.fossil".to_string(),
        );
        let b = fossil_base::SourceFile::new(
            &db,
            "Org : ex:Organization from src\n    ex:title = .title\n".to_string(),
            "b.fossil".to_string(),
        );
        let ws = WorkspaceIndex::build(&db, &[a, b]);

        // `User` is defined only in file A; resolves from the union.
        let user = ws.resolve("User");
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].0, a);
        assert_eq!(user[0].1.kind, SymbolKind::Mapping);

        // `Org` is defined only in file B.
        let org = ws.resolve("Org");
        assert_eq!(org.len(), 1);
        assert_eq!(org[0].0, b);
    }

    #[test]
    fn resolves_a_prefix_declared_in_one_file() {
        let db = db();
        // Prefix declared in file A, conceptually used in file B (which carries
        // no declaration of its own) — open-files-as-workspace lets B see it.
        let a = fossil_base::SourceFile::new(
            &db,
            "prefix ex: <https://example.org/>\n".to_string(),
            "a.fossil".to_string(),
        );
        let b = fossil_base::SourceFile::new(
            &db,
            "Org : ex:Organization from src\n    ex:title = .title\n".to_string(),
            "b.fossil".to_string(),
        );
        let ws = WorkspaceIndex::build(&db, &[a, b]);

        let hits = ws.resolve_prefix("ex");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, a);
        assert_eq!(hits[0].1, "https://example.org/");
    }

    #[test]
    fn resolve_returns_all_matching_definitions() {
        let db = db();
        // Same mapping name in two files — resolve returns BOTH (ambiguity is
        // the caller's to surface; the index does not silently drop one).
        let a = fossil_base::SourceFile::new(
            &db,
            "User : ex:Person from u\n    ex:name = .name\n".to_string(),
            "a.fossil".to_string(),
        );
        let b = fossil_base::SourceFile::new(
            &db,
            "User : ex:Agent from u\n    ex:label = .label\n".to_string(),
            "b.fossil".to_string(),
        );
        let ws = WorkspaceIndex::build(&db, &[a, b]);
        assert_eq!(ws.resolve("User").len(), 2);
    }

    #[test]
    fn symbols_of_returns_the_per_file_index() {
        let db = db();
        let a = fossil_base::SourceFile::new(
            &db,
            "User : ex:Person from u\n    ex:name = .name\n".to_string(),
            "a.fossil".to_string(),
        );
        let ws = WorkspaceIndex::build(&db, &[a]);
        let sym = ws.symbols_of(a).expect("file a is indexed");
        assert!(sym.lookup("User").is_some());
    }
}
