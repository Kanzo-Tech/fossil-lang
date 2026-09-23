//! Hover, completion and goto-definition on the **main thread** — the third
//! wire over the same `fossil-ide` answers.
//!
//! # Why a third wire, when `lsp_worker` already dispatches all three
//!
//! [`crate::lsp_worker`] is a `postMessage` transport, and a transport needs
//! somebody on the other end of it: a Worker that owns its own
//! [`crate::FossilPlayground`], a JSON-RPC client, an id table, and a second
//! copy of every buffer to keep the two workspaces in step. That is an LSP
//! client, it is a real piece of work, and a browser tab that already calls
//! `check()` synchronously does not need one to answer *what is the type under
//! this cursor*.
//!
//! So these are ordinary method calls, and the surface stays the shape the rest
//! of this crate has: `(handle, line, character)` in, a serialisable row out.
//! The Worker keeps every one of its routes; nothing here replaces it.
//! What is shared is what `/docs/design/three-hosts` says is shared — the
//! ANSWER, as a `fossil-ide` free function — and
//! `crates/fossil-wasm/tests/main_thread_parity.rs` is the test that crosses
//! the new wire rather than the function, because that page's other rule is
//! that two hosts sharing a function still disagree if they serialise it
//! differently.
//!
//! # These three take a SHARED borrow, and that is the whole re-entrancy story
//!
//! Hover fires on mouse-move, completion on nearly every keystroke, and the
//! checker on a 120 ms debounce — three different rates against one workspace,
//! which is exactly the arrangement that poisoned a session before
//! (`WasmPlayground`'s type-level note has the defect). It cannot recur here,
//! and not because of scheduling: **none of these three mutates**. Each takes
//! `try_borrow`, and shared borrows nest, so a hover during a live `check` — or
//! two of them at once — returns an answer rather than an error. Only
//! `update_file` takes `try_borrow_mut`, so the exclusive borrow is held on the
//! one path a host already coalesces.
//!
//! The consequence a caller owes is the other half: these read the text of the
//! LAST `update_file`, so a position query issued mid-debounce answers about
//! text one keystroke old and its ranges land one keystroke wrong. The host
//! pushes the buffer before it asks — `apps/playground/src/check.ts` does it
//! with a string compare, so the common case (nothing changed since the check)
//! costs one comparison and takes no exclusive borrow at all. That is what an
//! LSP client's ordering guarantee buys for free and a direct caller has to
//! arrange.
//!
//! # No `Option` field crosses this wire
//!
//! `serde_json` and `serde_wasm_bindgen` disagree about an absent optional
//! field, and each host's test asserts on its own side of the wire — the
//! measured defect behind three-hosts' second rule. The rows below have no
//! `Option` fields: a kind nobody set is `""` and a detail nobody wrote is
//! `""`. The one nullable thing on the surface is hover itself, which is
//! `Option<HoverRow>` because "no type under this cursor" is the ordinary
//! answer, and the TS wrapper normalises `undefined` to `null` where it lands.

use fossil_base::SourceFile;
use lsp_types::{CompletionItemKind, Range};

use crate::FossilPlayground;

/// What is under the cursor, rendered — the payload of `textDocument/hover`
/// with the LSP envelope taken off.
///
/// `markdown` is [`fossil_ide::hover_bidirectional`]'s: a ```` ```fossil ````
/// fence, the source-side type and where it came from, and — when the program
/// names an output document that resolves — a second block with the type the
/// shape demands of that predicate. `range` is UTF-16, because a JS host counts
/// in UTF-16 and LSP does too.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HoverRow {
    pub markdown: String,
    pub range: Range,
}

/// One completion candidate.
///
/// # `kind` is a NAME, and that is the whole reason this type exists
///
/// LSP spells `CompletionItemKind` as an integer, and the predecessor of
/// `packages/codemirror-fossil` is what happens when a number crosses this
/// boundary: it hard-copied the lexer's discriminants into a TS enum and was
/// wrong in nine places by the time it was deleted. The numbers here are the
/// LSP spec's rather than fossil's, so they are not going to be renumbered —
/// but the table that reads them would still live in TypeScript, where nothing
/// can check it. [`kind_name`] puts it in Rust, where
/// `every_lsp_kind_has_a_name` does.
///
/// The name is the LSP constant, lowercased (`FUNCTION` → `"function"`,
/// `ENUM_MEMBER` → `"enum_member"`). Mapping it onto whatever vocabulary an
/// editor draws icons from is that editor's layer's job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CompletionRow {
    pub label: String,
    /// The LSP kind, by name; `""` when the item carries none.
    pub kind: String,
    /// The signature, the shape property's IRI, the source field's type —
    /// whatever `fossil-ide` wrote beside the label. `""` when it wrote none.
    pub detail: String,
}

/// One place a definition is.
///
/// `uri` is the registry key VERBATIM — the path the host opened the buffer
/// under, which in a browser is usually a bare name like `hello.shex` and not a
/// URI. Same choice as [`crate::CheckRow::uri`] and for the same reason: a
/// caller matching this against the buffer it opened has to find the same
/// string it passed in. The LSP wire converts, because LSP will not accept
/// anything else; this surface has no wire to satisfy.
///
/// A target in ANOTHER file is the ordinary case rather than the exception —
/// two of the three positions goto-def recognises resolve into the shape
/// document — so a host with one editor pane still has to read `uri` before it
/// moves a cursor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DefinitionRow {
    pub uri: String,
    pub range: Range,
}

/// The LSP `CompletionItemKind` constant, by name.
///
/// Total over the twenty-five the specification defines; `None` and anything
/// outside them is `""`, which a host reads as "no icon".
#[must_use]
pub const fn kind_name(kind: Option<CompletionItemKind>) -> &'static str {
    let Some(kind) = kind else { return "" };
    match kind {
        CompletionItemKind::TEXT => "text",
        CompletionItemKind::METHOD => "method",
        CompletionItemKind::FUNCTION => "function",
        CompletionItemKind::CONSTRUCTOR => "constructor",
        CompletionItemKind::FIELD => "field",
        CompletionItemKind::VARIABLE => "variable",
        CompletionItemKind::CLASS => "class",
        CompletionItemKind::INTERFACE => "interface",
        CompletionItemKind::MODULE => "module",
        CompletionItemKind::PROPERTY => "property",
        CompletionItemKind::UNIT => "unit",
        CompletionItemKind::VALUE => "value",
        CompletionItemKind::ENUM => "enum",
        CompletionItemKind::KEYWORD => "keyword",
        CompletionItemKind::SNIPPET => "snippet",
        CompletionItemKind::COLOR => "color",
        CompletionItemKind::FILE => "file",
        CompletionItemKind::REFERENCE => "reference",
        CompletionItemKind::FOLDER => "folder",
        CompletionItemKind::ENUM_MEMBER => "enum_member",
        CompletionItemKind::CONSTANT => "constant",
        CompletionItemKind::STRUCT => "struct",
        CompletionItemKind::EVENT => "event",
        CompletionItemKind::OPERATOR => "operator",
        CompletionItemKind::TYPE_PARAMETER => "type_parameter",
        _ => "",
    }
}

impl FossilPlayground {
    /// Native-reachable hover — the pure-Rust half of
    /// [`crate::WasmPlayground::hover`].
    ///
    /// `None` for an unknown handle AND for a cursor with no type-bearing
    /// expression under it, which are the same answer to a host: nothing to
    /// show. The worker's route collapses them the same way.
    #[must_use]
    pub fn hover_row(
        &self,
        handle: crate::FileHandle,
        line: u32,
        character: u32,
    ) -> Option<HoverRow> {
        let db = self.base_db();
        let file = self.file_by_handle(handle)?;
        let info = fossil_ide::hover_bidirectional(db, file, line, character)?;
        let index = fossil_ide::line_index(db, file);
        Some(HoverRow {
            markdown: info.markdown,
            range: fossil_ide::byte_range_to_range(&index, info.range),
        })
    }

    /// Native-reachable completion — the pure-Rust half of
    /// [`crate::WasmPlayground::completions`].
    ///
    /// An unknown handle is an empty list rather than an error: a completion
    /// request racing a `closeFile` is a normal thing for an editor to do, and
    /// there is nothing to offer either way.
    #[must_use]
    pub fn completion_rows(
        &self,
        handle: crate::FileHandle,
        line: u32,
        character: u32,
    ) -> Vec<CompletionRow> {
        let db = self.base_db();
        let Some(file) = self.file_by_handle(handle) else {
            return Vec::new();
        };
        let files: Vec<SourceFile> = self.open_source_files();
        fossil_ide::completions(db, &files, file, line, character)
            .into_iter()
            .map(|item| CompletionRow {
                label: item.label,
                kind: kind_name(item.kind).to_string(),
                detail: item.detail.unwrap_or_default(),
            })
            .collect()
    }

    /// Native-reachable goto-definition — the pure-Rust half of
    /// [`crate::WasmPlayground::goto_definition`].
    ///
    /// Empty when the handle is unknown, when nothing is under the cursor, and
    /// when the thing under it has no definition anywhere in the open set —
    /// three cases a host renders identically.
    #[must_use]
    pub fn definition_rows(
        &self,
        handle: crate::FileHandle,
        line: u32,
        character: u32,
    ) -> Vec<DefinitionRow> {
        let db = self.base_db();
        let Some(file) = self.file_by_handle(handle) else {
            return Vec::new();
        };
        let files: Vec<SourceFile> = self.open_source_files();
        fossil_ide::goto_definition(db, &files, file, line, character)
            .into_iter()
            .map(|target| DefinitionRow {
                uri: target.file.path(db).clone(),
                range: fossil_ide::byte_range_to_range(
                    &fossil_ide::line_index(db, target.file),
                    target.range,
                ),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{CompletionItemKind, kind_name};

    /// The twenty-five the LSP specification defines, in its own order. Written
    /// out because the point is that the table is TOTAL: a constant missing
    /// from `kind_name`'s match falls through to `""`, which a host renders as
    /// no icon and nothing else notices.
    const ALL: [CompletionItemKind; 25] = [
        CompletionItemKind::TEXT,
        CompletionItemKind::METHOD,
        CompletionItemKind::FUNCTION,
        CompletionItemKind::CONSTRUCTOR,
        CompletionItemKind::FIELD,
        CompletionItemKind::VARIABLE,
        CompletionItemKind::CLASS,
        CompletionItemKind::INTERFACE,
        CompletionItemKind::MODULE,
        CompletionItemKind::PROPERTY,
        CompletionItemKind::UNIT,
        CompletionItemKind::VALUE,
        CompletionItemKind::ENUM,
        CompletionItemKind::KEYWORD,
        CompletionItemKind::SNIPPET,
        CompletionItemKind::COLOR,
        CompletionItemKind::FILE,
        CompletionItemKind::REFERENCE,
        CompletionItemKind::FOLDER,
        CompletionItemKind::ENUM_MEMBER,
        CompletionItemKind::CONSTANT,
        CompletionItemKind::STRUCT,
        CompletionItemKind::EVENT,
        CompletionItemKind::OPERATOR,
        CompletionItemKind::TYPE_PARAMETER,
    ];

    #[test]
    fn every_lsp_kind_has_a_name() {
        for kind in ALL {
            assert!(
                !kind_name(Some(kind)).is_empty(),
                "CompletionItemKind {kind:?} falls through kind_name's match — \
                 an editor would draw no icon for it and nothing else would say so"
            );
        }
    }

    #[test]
    fn names_are_distinct() {
        let mut names: Vec<&str> = ALL.iter().map(|k| kind_name(Some(*k))).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "two kinds share a name: {names:?}");
    }

    #[test]
    fn no_kind_is_the_empty_name() {
        assert_eq!(kind_name(None), "");
    }
}
