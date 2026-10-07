//! Hover, completion, goto-definition and semantic tokens on the **main
//! thread** — the browser's one wire over the `fossil-ide` answers the native
//! LSP serves.
//!
//! # Why method calls and not an LSP transport
//!
//! A `postMessage` LSP transport needs somebody on the other end of it: a
//! Worker that owns its own [`crate::FossilWorkspace`], a JSON-RPC client, an
//! id table, and a second copy of every buffer to keep the two workspaces in
//! step. That is an LSP client, it is a real piece of work, and a browser tab
//! that already calls `diagnostics()` synchronously does not need one to answer
//! *what is the type under this cursor*. There was such a Worker here, with no
//! consumer, and it is deleted.
//!
//! So these are ordinary method calls, and the surface stays the shape the rest
//! of this crate has: `(handle, line, character)` in, a serialisable row out.
//! What is shared is what `/docs/design/three-hosts` says is shared — the
//! ANSWER, as a `fossil-ide` free function.
//!
//! # These four take a SHARED borrow, and that is the whole re-entrancy story
//!
//! Hover fires on mouse-move, completion and semantic tokens on nearly every
//! keystroke, and the checker on a 120 ms debounce — three different rates
//! against one workspace,
//! which is exactly the arrangement that poisoned a session before
//! (`WasmWorkspace`'s type-level note has the defect). It cannot recur here,
//! and not because of scheduling: **none of these four mutates**. Each takes
//! `try_borrow`, and shared borrows nest, so a hover during a live `diagnostics` — or
//! two of them at once — returns an answer rather than an error. Only
//! `update_file` takes `try_borrow_mut`, so the exclusive borrow is held on the
//! one path a host already coalesces.
//!
//! The consequence a caller owes is the other half: these read the text of the
//! LAST `update_file`, so a position query issued mid-debounce answers about
//! text one keystroke old and its ranges land one keystroke wrong. The host
//! pushes the buffer before it asks — `@fossil-lang/codemirror-fossil` pushes
//! it from the view's update cycle, synchronously inside the transaction that
//! changed it, so no query can read a newer document than the workspace holds.
//! That is what an LSP client's `didChange` ordering buys for free and a direct
//! caller has to arrange.
//!
//! # No `Option` field crosses this wire
//!
//! `serde_json` and `serde_wasm_bindgen` disagree about an absent optional
//! field, and each host's test asserts on its own side of the wire — the
//! measured defect behind three-hosts' second rule. The rows below have no
//! `Option` fields: a kind nobody set is `""` and a detail nobody wrote is
//! `""`. The one nullable thing on the surface is hover itself, which is
//! `Option<Hover>` because "no type under this cursor" is the ordinary
//! answer, and the TS wrapper normalises `undefined` to `null` where it lands.

use fossil_base::SourceFile;
use lsp_types::{CompletionItemKind, Range};

use crate::FossilWorkspace;

/// What is under the cursor, rendered — the payload of `textDocument/hover`
/// with the LSP envelope taken off.
///
/// `markdown` is [`fossil_ide::hover()`]'s: a ```` ```fossil ````
/// fence, the source-side type and where it came from, and — when the program
/// names an output document that resolves — a second block with the type the
/// shape demands of that predicate. `range` is UTF-16, because a JS host counts
/// in UTF-16 and LSP does too.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct Hover {
    pub markdown: String,
    #[schemars(with = "fossil_ide::wire::RangeSchema")]
    pub range: Range,
}

/// What a completion candidate is: the two LSP `CompletionItemKind`s
/// `fossil-ide` emits, by their LSP names.
///
/// A name and not a number, because the predecessor of
/// `packages/codemirror-fossil` is what happens when a number crosses this
/// boundary: it hard-copied the lexer's discriminants into a TS enum and was
/// wrong in nine places by the time it was deleted. And these two and not the
/// specification's twenty-five, because a variant nothing emits is a row of a
/// table on each side of the boundary that nothing can exercise. A third kind
/// is a variant here, and the generated union makes the editor's table say
/// what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CompletionKind {
    Function,
    Field,
}

impl CompletionKind {
    const fn of(kind: Option<CompletionItemKind>) -> Option<Self> {
        match kind {
            Some(CompletionItemKind::FUNCTION) => Some(Self::Function),
            Some(CompletionItemKind::FIELD) => Some(Self::Field),
            _ => None,
        }
    }
}

/// One completion candidate — LSP's `CompletionItem`, with the fields
/// `fossil-ide` fills.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    /// Absent when the item carries no kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<CompletionKind>,
    /// The signature, the shape property's IRI, the source field's type —
    /// whatever `fossil-ide` wrote beside the label. `""` when it wrote none.
    pub detail: String,
    /// The text a pick writes, which is the label's one spelling: the label
    /// itself, or `"Person.id"` for a column whose name is not an identifier
    /// (`fossil_syntax::name`). Never empty and never absent.
    pub insert_text: String,
}

/// One place a definition is.
///
/// `uri` is the registry key VERBATIM — the path the host opened the buffer
/// under, which in a browser is usually a bare name like `hello.shex` and not a
/// URI. Same choice as [`crate::Diagnostic::uri`] and for the same reason: a
/// caller matching this against the buffer it opened has to find the same
/// string it passed in. The LSP wire converts, because LSP will not accept
/// anything else; this surface has no wire to satisfy.
///
/// A target in ANOTHER file is the ordinary case rather than the exception —
/// two of the three positions goto-def recognises resolve into the shape
/// document — so a host with one editor pane still has to read `uri` before it
/// moves a cursor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct Location {
    pub uri: String,
    #[schemars(with = "fossil_ide::wire::RangeSchema")]
    pub range: Range,
}

/// One classified span of the program — `textDocument/semanticTokens/full`
/// as absolute rows rather than the LSP delta stream.
///
/// `kind` and `modifiers` are the legend's NAMES (`"type"`, `"declaration"`),
/// for the reason [`CompletionKind`] is one: the number is an index into a
/// table, and the table belongs on the side that can check it. `range` is
/// UTF-16, like every other range on this surface, and unlike the LSP stream it
/// may cross a line — a multi-line string is one row.
///
/// The rows are in source order and never overlap; a connection reference is
/// carved out of its string literal, so `"@warehouse/x.csv"` is three rows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct SemanticToken {
    #[schemars(with = "fossil_ide::wire::RangeSchema")]
    pub range: Range,
    pub kind: String,
    /// Empty when the span carries none — never absent.
    pub modifiers: Vec<String>,
}

impl FossilWorkspace {
    /// Native-reachable hover — the pure-Rust half of
    /// [`crate::WasmWorkspace::hover`].
    ///
    /// `None` for an unknown handle AND for a cursor with no type-bearing
    /// expression under it, which are the same answer to a host: nothing to
    /// show. The worker's route collapses them the same way.
    #[must_use]
    pub fn hover_row(&self, handle: crate::FileHandle, line: u32, character: u32) -> Option<Hover> {
        let db = self.base_db();
        let file = self.file_by_handle(handle)?;
        let info = fossil_ide::hover(db, file, line, character)?;
        let index = fossil_ide::line_index(db, file);
        Some(Hover {
            markdown: info.markdown,
            range: fossil_ide::range(index, info.range),
        })
    }

    /// Native-reachable completion — the pure-Rust half of
    /// [`crate::WasmWorkspace::completions`].
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
    ) -> Vec<CompletionItem> {
        let db = self.base_db();
        let Some(file) = self.file_by_handle(handle) else {
            return Vec::new();
        };
        let files: Vec<SourceFile> = self.open_source_files();
        fossil_ide::completions(db, &files, file, line, character)
            .into_iter()
            .map(|item| CompletionItem {
                insert_text: item.insert_text.unwrap_or_else(|| item.label.clone()),
                label: item.label,
                kind: CompletionKind::of(item.kind),
                detail: item.detail.unwrap_or_default(),
            })
            .collect()
    }

    /// Native-reachable semantic tokens — the pure-Rust half of
    /// [`crate::WasmWorkspace::semantic_tokens`].
    ///
    /// An unknown handle is an empty list: a highlighter racing a `closeFile`
    /// has nothing to paint either way.
    #[must_use]
    pub fn semantic_token_rows(&self, handle: crate::FileHandle) -> Vec<SemanticToken> {
        let db = self.base_db();
        let Some(file) = self.file_by_handle(handle) else {
            return Vec::new();
        };
        let index = fossil_ide::line_index(db, file);
        fossil_ide::semantic_spans(db, file)
            .into_iter()
            .map(|span| SemanticToken {
                range: fossil_ide::range(index, span.range),
                kind: span.token_type.name().to_string(),
                modifiers: fossil_ide::modifier_names(span.modifiers)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            })
            .collect()
    }

    /// Native-reachable goto-definition — the pure-Rust half of
    /// [`crate::WasmWorkspace::goto_definition`].
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
    ) -> Vec<Location> {
        let db = self.base_db();
        let Some(file) = self.file_by_handle(handle) else {
            return Vec::new();
        };
        let files: Vec<SourceFile> = self.open_source_files();
        fossil_ide::goto_definition(db, &files, file, line, character)
            .into_iter()
            .map(|target| Location {
                uri: target.file.path(db).clone(),
                range: fossil_ide::range(fossil_ide::line_index(db, target.file), target.range),
            })
            .collect()
    }
}
