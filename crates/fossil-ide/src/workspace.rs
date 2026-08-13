//! [`WorkspaceIndex`] — cross-file symbol aggregation.
//!
//! Aggregates the per-file [`SymbolIndex`]es of a SET of [`SourceFile`]s into a
//! single resolver, so a symbol declared in file A resolves when referenced
//! from file B. This implements the **open-files-as-workspace** model: the
//! file set is whatever the host holds open — the LSP's
//! `LspState.files` map or the playground's multi-panel set — NOT a filesystem
//! scan of a workspace root (deferred to v2; conflicts with the WASM
//! `VirtualFS`).
//!
//! Like [`SymbolIndex`] this is a plain struct (no Salsa query of its own), so
//! it adds zero tracked queries and keeps the per-mapping `body()` fan-out at 1
//! (Research Pitfall #3). It is rebuilt from the open-file set on demand; the
//! per-file `SymbolIndex::build` underneath is itself cheap (one CST walk).

use fossil_base::SourceFile;

use crate::symbol_index::{SymbolEntry, SymbolIndex};

/// Cross-file symbol resolver over the open-file set.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceIndex {
    files: Vec<(SourceFile, SymbolIndex)>,
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
                (file, symbols)
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
        for (file, symbols) in &self.files {
            for entry in symbols.entries() {
                if entry.name.as_str() == name {
                    hits.push((*file, entry.clone()));
                }
            }
        }
        hits
    }

    // `resolve_prefix` lived here — `(file, iri)` for every declaration of a
    // prefix across the open files. There are no prefix declarations
    // and no CURIE for one to expand, so the third element of each tuple above
    // went with it and this index is symbols only.

    /// The per-file [`SymbolIndex`] for one open file, if present.
    #[must_use]
    pub fn symbols_of(&self, file: SourceFile) -> Option<&SymbolIndex> {
        self.files
            .iter()
            .find(|(f, _)| *f == file)
            .map(|(_, s)| s)
    }

    /// The set of files this index spans.
    pub fn files(&self) -> impl Iterator<Item = SourceFile> + '_ {
        self.files.iter().map(|(f, _)| *f)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::symbol_index::SymbolKind;
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    #[test]
    fn resolves_a_mapping_across_two_files() {
        let db = db();
        let a = fossil_base::SourceFile::new(
            &db,
            "Users : Person from User\n    name = User.name\n".to_string(),
            "a.fossil".to_string(),
        );
        let b = fossil_base::SourceFile::new(
            &db,
            "Org : Organization from Src\n    title = Src.title\n".to_string(),
            "b.fossil".to_string(),
        );
        let ws = WorkspaceIndex::build(&db, &[a, b]);

        // `Users` is defined only in file A; resolves from the union.
        let user = ws.resolve("Users");
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].0, a);
        assert_eq!(user[0].1.kind, SymbolKind::Mapping);

        // `Org` is defined only in file B.
        let org = ws.resolve("Org");
        assert_eq!(org.len(), 1);
        assert_eq!(org[0].0, b);
    }

    // `resolves_a_prefix_declared_in_one_file` lived here: a `prefix` line in
    // file A, resolved from file B, proving open-files-as-workspace. Both the
    // production and the form it indexed are gone; `resolves_a_mapping_across_two_files`
    // above still proves the cross-file half.

    #[test]
    fn resolve_returns_all_matching_definitions() {
        let db = db();
        // Same mapping name in two files — resolve returns BOTH (ambiguity is
        // the caller's to surface; the index does not silently drop one).
        let a = fossil_base::SourceFile::new(
            &db,
            "User : Person from u\n    name = u.name\n".to_string(),
            "a.fossil".to_string(),
        );
        let b = fossil_base::SourceFile::new(
            &db,
            "User : Agent from u\n    label = u.label\n".to_string(),
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
            "User : Person from u\n    name = u.name\n".to_string(),
            "a.fossil".to_string(),
        );
        let ws = WorkspaceIndex::build(&db, &[a]);
        let sym = ws.symbols_of(a).expect("file a is indexed");
        assert!(sym.lookup("User").is_some());
    }
}
