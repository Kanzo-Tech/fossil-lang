//! `textDocument/documentSymbol` — the document outline.
//!
//! One entry per top-level definition, in written order, read from
//! [`fossil_hir::def_map::DefMap::definitions`] — the table every other
//! feature resolves against, so the outline lists exactly the names
//! goto-definition can land on. The list is **flat**: a mapping's properties
//! are not definitions the def map holds, and nesting them is a refinement
//! nothing has asked for.
//!
//! Each symbol's `range` is the whole item and its `selection_range` the name,
//! converted to UTF-16 through the [`crate::position::LineIndex`]: Monaco counts UTF-16, so a
//! byte range would mis-highlight after a multi-byte character.
//!
//! # Kind mapping
//!
//! | Namespace | LSP `SymbolKind` | rationale                           |
//! |-----------|------------------|-------------------------------------|
//! | `Mapping` | `CLASS`          | a mapping produces typed subjects    |
//! | `Type`    | `INTERFACE`      | a type is a structural contract      |
//! | `Value`   | `VARIABLE`       | a `:=` binds a name to a value       |
//!
//! WASM-clean: returns `lsp_types::DocumentSymbol` directly (`lsp-types` itself
//! is wasm32-clean), so the wasm workspace consumes it with no translation.

use fossil_base::SourceFile;
use fossil_hir::def_map::{Namespace, def_map};
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::position::{line_index, range};

/// Build the document outline for `file`: a flat `Vec<DocumentSymbol>` in
/// source order, one entry per top-level definition (source / type / mapping).
///
/// Returns an empty `Vec` for a file with no top-level definitions.
#[must_use]
pub fn document_symbols(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<DocumentSymbol> {
    let li = line_index(db, file);
    def_map(db, file)
        .definitions(db)
        .into_iter()
        .map(|def| {
            #[allow(deprecated)] // `deprecated` field is required by the lsp-types struct literal
            DocumentSymbol {
                name: def.name().to_string(),
                detail: None,
                kind: kind_of(def.namespace()),
                tags: None,
                deprecated: None,
                range: range(li, def.span()),
                selection_range: range(li, def.name_span()),
                children: None,
            }
        })
        .collect()
}

/// The LSP kind of a definition, by the table it lives in (see the module
/// table).
const fn kind_of(namespace: Namespace) -> SymbolKind {
    match namespace {
        Namespace::Mapping => SymbolKind::CLASS,
        Namespace::Type => SymbolKind::INTERFACE,
        // A `:=` binding is a name bound to a value, which is what `VARIABLE`
        // is for; `OBJECT` and `FILE` both say something about what the
        // right-hand side READS, which the outline does not know —
        // `Adults := User.where(…)` opens no file.
        Namespace::Value => SymbolKind::VARIABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db_file(src: &str) -> (fossil_base::FossilDb, SourceFile) {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    const FIXTURE: &str = "\
type { Person } := io.shex(\"person.shex\")

users := io.csv(\"users.csv\")

User : Person from users
    name = users.name
";

    /// The mapping, and the type the program binds — at its binding, not at
    /// the header that names it.
    #[test]
    fn outline_has_the_mapping_and_the_type_it_binds() {
        let (db, file) = db_file(FIXTURE);
        let syms = document_symbols(&db, file);
        assert!(
            syms.iter()
                .any(|s| s.kind == SymbolKind::CLASS && s.name == "User"),
            "expected a CLASS for the `User` mapping: {syms:?}"
        );
        let person = syms
            .iter()
            .find(|s| s.kind == SymbolKind::INTERFACE && s.name == "Person")
            .unwrap_or_else(|| panic!("expected an INTERFACE for `Person`: {syms:?}"));
        assert_eq!(
            person.selection_range.start.line, 0,
            "`Person` is declared by the `type` binding on line 0",
        );
    }

    /// The item is the range and the name the selection, inside it — what
    /// LSP requires of a `DocumentSymbol`.
    #[test]
    fn the_selection_is_the_name_inside_the_item() {
        let (db, file) = db_file(FIXTURE);
        let text = file.text(&db);
        let li = line_index(&db, file);
        for s in document_symbols(&db, file) {
            let nonempty = s.range.end.line > s.range.start.line
                || s.range.end.character > s.range.start.character;
            assert!(nonempty, "empty range for {}", s.name);
            assert!(
                s.range.start <= s.selection_range.start && s.selection_range.end <= s.range.end,
                "the selection must lie inside the range for {}",
                s.name,
            );
            let name = crate::position::span(li, s.selection_range);
            assert_eq!(&text[name.start as usize..name.end as usize], s.name);
        }
    }

    #[test]
    fn empty_file_has_no_symbols() {
        let (db, file) = db_file("");
        assert!(document_symbols(&db, file).is_empty());
    }
}
