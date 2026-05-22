//! `textDocument/definition` — cross-file goto-def (SC#4).
//!
//! Resolves the identifier under the cursor to its definition site(s) across the
//! **open-file set** (the open-files-as-workspace model, ADR-0023). Unlike hover
//! (which reads per-mapping type provenance), goto-def is a pure name-resolution
//! query: it classifies the token under the cursor and looks it up in the
//! [`fossil_ide_db::WorkspaceIndex`] (built by 06-03), returning every matching
//! definition's owning file + byte range.
//!
//! # The four symbol kinds (SC#4)
//!
//! 1. **prefix** — the `ex` in `ex:Person` / `ex:name`, or a bare prefix token.
//!    Resolved via [`WorkspaceIndex::resolve_prefix`] to the declaring
//!    `prefix ex: <...>` site (cross-file: declared in file A, used in file B).
//! 2. **mapping** — a mapping-header name (`User`). Resolved via
//!    [`WorkspaceIndex::resolve`] to the `User : ... from ...` header.
//! 3. **function** — a top-level (`@export f := …`) definition name. Same
//!    `resolve` path.
//! 4. **shape ref** — the `ex:Person` prefixed name in a mapping header. The
//!    06-03 [`fossil_ide_db::SymbolIndex`] records shape refs under their full
//!    surface text (`"ex:Person"`), so `resolve` lands on the declaring mapping
//!    site.
//!
//! # Domain boundary (Research §fossil-ide split)
//!
//! Returns Fossil-domain [`NavigationTarget`]s (`{ file, range: Range<u32> }`),
//! NEVER `lsp_types::Location` — the byte→UTF-16 range translation happens in
//! `fossil-lsp` (06-08) via [`crate::position::offset_to_lsp_position`], so this
//! function stays transport-free and WASM-clean. No new Salsa query is added:
//! the `WorkspaceIndex` is a plain struct built by a CST walk, so the
//! per-mapping `body()` fan-out is unchanged (Research Pitfall #3).

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_ide_db::WorkspaceIndex;

use crate::position::token_at_position;

/// A goto-def navigation target: the owning file + the byte range of the
/// definition site. `fossil-lsp` (06-08) translates `range` to a UTF-16
/// `lsp_types::Range` and pairs it with the file URI to build a
/// `lsp_types::Location`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    /// The file that contains the definition.
    pub file: SourceFile,
    /// Byte range of the definition site into that file's text.
    pub range: Range<u32>,
}

/// Resolve the definition(s) of the identifier at an LSP position across the
/// open-file set.
///
/// `files` is the host's open-file set (the LSP `LspState.files` or the
/// playground's multi-panel set); `file` is the file the cursor is in. Returns
/// ALL matching definition sites (a name may be declared in more than one open
/// file — the index does not silently drop ambiguity; the caller may pick the
/// first or surface a disambiguation). Returns an empty `Vec` when the cursor is
/// not on a resolvable identifier (whitespace, punctuation, an unknown name).
///
/// `line` / `character` are UTF-16 LSP coordinates (resolved via the
/// [`crate::line_index::LineIndex`]); never byte offsets.
#[must_use]
pub fn goto_definition(
    db: &dyn fossil_base::Db,
    files: &[SourceFile],
    file: SourceFile,
    line: u32,
    character: u32,
) -> Vec<NavigationTarget> {
    let Some(token) = token_at_position(db, file, line, character) else {
        return Vec::new();
    };
    let raw = token.text();

    let ws = WorkspaceIndex::build(db, files);
    let mut targets = Vec::new();

    // 1. Prefix use: the cursor is on the prefix segment of a `prefix:local`
    //    name (`ex` in `ex:Person`), or on a bare prefix token. The token under
    //    the cursor is the bare prefix IDENT; resolve it to its declaring
    //    `prefix ... ` site. We also handle the case where the lexer hands back
    //    the whole `ex:Person` PNAME token by splitting on the first `:`.
    let prefix_candidate = raw.split(':').next().unwrap_or(raw);
    if !prefix_candidate.is_empty() {
        for (decl_file, _iri) in ws.resolve_prefix(prefix_candidate) {
            if let Some(range) = prefix_decl_range(db, decl_file, prefix_candidate) {
                push_unique(
                    &mut targets,
                    NavigationTarget {
                        file: decl_file,
                        range,
                    },
                );
            }
        }
    }

    // 2/3/4. Mapping / function / shape-ref names. The shape index entries are
    //    keyed by their full surface text (`ex:Person`), so resolving the raw
    //    token text covers shape refs directly; mapping + function names resolve
    //    by their bare IDENT.
    for (decl_file, entry) in ws.resolve(raw) {
        push_unique(
            &mut targets,
            NavigationTarget {
                file: decl_file,
                range: entry.range,
            },
        );
    }

    targets
}

/// Find the byte range of the `prefix <name>: <iri>` declaration in `file`.
///
/// The [`fossil_ide_db::SymbolIndex`] records prefix declarations under their
/// `SymbolKind::Prefix` entries with the declaration's byte range; we re-read
/// the per-file index to obtain it (the `resolve_prefix` API returns the IRI,
/// not the range, since most callers only need the IRI for auto-import).
fn prefix_decl_range(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    prefix: &str,
) -> Option<Range<u32>> {
    use fossil_ide_db::{SymbolIndex, SymbolKind};
    let idx = SymbolIndex::build(db, file);
    idx.of_kind(SymbolKind::Prefix)
        .find(|e| e.name.as_str() == prefix)
        .map(|e| e.range.clone())
}

/// Push a target only if an equal one is not already present (a name may match
/// both a prefix-resolve and a symbol-resolve in pathological cases; de-dup so
/// the LSP does not show a doubled location).
fn push_unique(targets: &mut Vec<NavigationTarget>, target: NavigationTarget) {
    if !targets.contains(&target) {
        targets.push(target);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        fossil_base::FossilDb::new(system)
    }

    fn file(db: &fossil_base::FossilDb, name: &str, src: &str) -> SourceFile {
        SourceFile::new(db, src.to_string(), name.to_string())
    }

    #[test]
    fn resolves_a_mapping_in_the_same_file() {
        let db = db();
        let src = "\
prefix ex: <https://example.org/>
User : ex:Person from users
    ex:name = .name
";
        let f = file(&db, "a.fossil", src);
        // Cursor on `User` (line 1, col 1).
        let hits = goto_definition(&db, &[f], f, 1, 1);
        assert!(
            hits.iter().any(|t| t.file == f),
            "User must resolve to its mapping header in the same file; got {hits:?}",
        );
    }

    #[test]
    fn resolves_a_prefix_declared_in_another_file() {
        let db = db();
        let a = file(&db, "a.fossil", "prefix ex: <https://example.org/>\n");
        let b = file(
            &db,
            "b.fossil",
            "User : ex:Person from users\n    ex:name = .name\n",
        );
        // In file B, cursor on the `ex` of `ex:Person` (line 0). The header is
        // `User : ex:Person from users`; `ex` starts at column 7.
        let hits = goto_definition(&db, &[a, b], b, 0, 8);
        assert!(
            hits.iter().any(|t| t.file == a),
            "the `ex` prefix must resolve cross-file to its declaration in file A; got {hits:?}",
        );
    }
}
