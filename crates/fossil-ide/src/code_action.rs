//! `textDocument/codeAction` — the three SC#5 quick-fixes.
//!
//! [`code_actions`] turns a request's `(file, range, diagnostics)` into a
//! `Vec<lsp_types::CodeAction>` (the `lsp-types`-direct shape confirmed
//! WASM-clean by 06-01 Spike A), one `QuickFix` per matching diagnostic. The
//! three actions Phase 6 SC#5 requires:
//!
//! 1. **did-you-mean** — a diagnostic carrying the structured
//!    [`fossil_base::DidYouMean`] candidate (Phase 3's `strsim` Levenshtein
//!    nearest, surfaced STRUCTURALLY by plan 06-08 Task 1) yields a `QuickFix`
//!    whose `WorkspaceEdit` replaces the typo's `wrong_span` with the
//!    `replacement`. Read from the typed field — NOT parsed from the message
//!    string (Research §code actions, ADR-0006 Approach A).
//! 2. **auto-import prefix** — an unknown-prefix diagnostic yields a `QuickFix`
//!    inserting a `prefix xx: <iri>` declaration at the top of the file. The
//!    IRI comes from [`fossil_ide_db::WELL_KNOWN_PREFIXES`] for rdf/rdfs/xsd/owl
//!    (via the 06-03 [`fossil_ide_db::PrefixIndex`]); an unknown prefix gets a
//!    `<>` placeholder the user fills in.
//! 3. **split-mapping** — a target `ShEx` `OneOf` diagnostic ALREADY carries the
//!    generated split-into-N-mappings snippet in
//!    [`fossil_base::Diagnostic::suggestion_source`] (Phase 3
//!    `generate_split_suggestion`, proven to re-compile by plan 03-08). The
//!    `QuickFix` reads `suggestion_source` DIRECTLY (Research `Don't-Hand-Roll`
//!    — never regenerate it) and replaces the offending mapping's range with it.
//!
//! # Domain + WASM boundary
//!
//! Returns `lsp_types::CodeAction` directly — no stdio / JSON-RPC. All edits use
//! UTF-16 LSP ranges (06-05 [`crate::line_index::LineIndex`]); the byte spans on
//! the incoming diagnostics are converted via the FILE-keyed line index, so no
//! new per-mapping Salsa query is added (Research Pitfall #3). No `Box<dyn>`; no
//! `TyKind::Unknown` ever reaches a title or edit (the action text is built from
//! the structured candidate / the pre-generated snippet, never from a type).

use std::collections::HashMap;
use std::str::FromStr as _;

use fossil_base::{Diagnostic, SourceFile, Span};
use fossil_ide_db::WELL_KNOWN_PREFIXES;
use lsp_types::{
    CodeAction, CodeActionKind, Diagnostic as LspDiagnostic, Position, Range, TextEdit, Uri,
    WorkspaceEdit,
};

use crate::line_index::{LineIndex, Utf16Position};
use crate::position::line_index;

/// Compute the SC#5 code actions for the diagnostics overlapping `range`.
///
/// `diagnostics` is the set the LSP `textDocument/codeAction` request passes in
/// the request params (the diagnostics the host already published for `file`);
/// each is matched against the three action triggers by the STRUCTURED fields it
/// carries (`did_you_mean`, the `undeclared prefix` message + span, and
/// `suggestion_source`). Only diagnostics whose span intersects `range` are
/// considered, mirroring the LSP "actions for the current selection" contract.
///
/// `range` is a UTF-16 LSP range; all produced edits are UTF-16 too.
#[must_use]
pub fn code_actions(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    range: Range,
    diagnostics: &[Diagnostic],
) -> Vec<CodeAction> {
    let index = line_index(db, file);
    let uri = file_uri(db, file);
    let sel = range_to_byte_span(&index, range);

    let mut actions = Vec::new();
    for diag in diagnostics {
        // Only offer actions for diagnostics overlapping the requested range.
        if !spans_overlap(diag.span, sel) {
            continue;
        }
        if let Some(a) = did_you_mean_action(&index, &uri, diag) {
            actions.push(a);
        }
        if let Some(a) = auto_import_action(db, file, &index, &uri, diag) {
            actions.push(a);
        }
        if let Some(a) = split_mapping_action(&index, &uri, diag) {
            actions.push(a);
        }
    }
    actions
}

/// Action 1: did-you-mean rename quick-fix. Reads the structured
/// [`fossil_base::DidYouMean`] candidate and replaces its `wrong_span` with the
/// `replacement` — no message parsing.
fn did_you_mean_action(index: &LineIndex, uri: &Uri, diag: &Diagnostic) -> Option<CodeAction> {
    let dym = diag.did_you_mean.as_ref()?;
    let edit = TextEdit::new(
        byte_span_to_range(index, dym.wrong_span),
        dym.replacement.clone(),
    );
    Some(quick_fix(
        format!("Replace with `{}`", dym.replacement),
        uri.clone(),
        vec![edit],
        diag,
        true,
    ))
}

/// Action 2: auto-import an unknown prefix. Triggered by the "undeclared
/// prefix" diagnostic (the message `lower.rs` emits). Inserts a
/// `prefix xx: <iri>` line at the top of the file — the canonical IRI for a
/// well-known prefix, a `<>` placeholder otherwise.
fn auto_import_action(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    index: &LineIndex,
    uri: &Uri,
    diag: &Diagnostic,
) -> Option<CodeAction> {
    let prefix = unknown_prefix_name(&diag.message)?;
    // Don't offer the import if the file already declares the prefix (a stale
    // diagnostic after the user fixed it manually).
    let prefixes = fossil_ide_db::PrefixIndex::build(db, file);
    if prefixes.is_declared(&prefix) {
        return None;
    }
    let iri = WELL_KNOWN_PREFIXES
        .iter()
        .find(|(p, _)| *p == prefix)
        .map_or(String::new(), |(_, iri)| (*iri).to_string());
    let line = format!("prefix {prefix}: <{iri}>\n");
    let edit = TextEdit::new(byte_span_to_range(index, Span::new(0, 0)), line);
    Some(quick_fix(
        format!("Import prefix `{prefix}:`"),
        uri.clone(),
        vec![edit],
        diag,
        true,
    ))
}

/// Action 3: split-mapping. Reads the pre-generated split snippet from
/// [`fossil_base::Diagnostic::suggestion_source`] (never regenerated) and
/// replaces the offending mapping's span with it.
fn split_mapping_action(index: &LineIndex, uri: &Uri, diag: &Diagnostic) -> Option<CodeAction> {
    let snippet = diag.suggestion_source.as_ref()?;
    let edit = TextEdit::new(byte_span_to_range(index, diag.span), snippet.clone());
    Some(quick_fix(
        "Split mapping into one per ShEx OneOf disjunct".to_string(),
        uri.clone(),
        vec![edit],
        diag,
        true,
    ))
}

/// Parse the prefix name out of the "undeclared prefix" diagnostic message
/// `lower.rs` emits. The STRUCTURED trigger is "this is an unknown-prefix
/// diagnostic"; the prefix name is the only datum the import line needs, and
/// `lower.rs` spells it in backticks immediately after the marker text.
fn unknown_prefix_name(message: &str) -> Option<String> {
    const MARKER: &str = "undeclared prefix `";
    let rest = message.strip_prefix(MARKER).or_else(|| {
        let idx = message.find(MARKER)?;
        Some(&message[idx + MARKER.len()..])
    })?;
    // The name runs up to the trailing `:` `` ` `` pair (`xx:` ).
    let name: String = rest
        .chars()
        .take_while(|c| *c != ':' && *c != '`')
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

/// Build a `quick fix` [`CodeAction`] resolving `diag` with one document's
/// worth of [`TextEdit`]s.
//
// `mutable_key_type`: clippy flags `HashMap<Uri, _>` because `lsp_types::Uri`
// wraps `fluent_uri::Uri` whose `Hash` clippy cannot prove is interior-
// mutability-free. The `WorkspaceEdit.changes` map IS keyed by `Uri` (that is
// the LSP wire shape we must produce); the key is never mutated after
// insertion, so the lint is a false positive here.
#[allow(clippy::mutable_key_type)]
fn quick_fix(
    title: String,
    uri: Uri,
    edits: Vec<TextEdit>,
    diag: &Diagnostic,
    is_preferred: bool,
) -> CodeAction {
    let mut changes = HashMap::new();
    changes.insert(uri, edits);
    CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![lsp_diagnostic_stub(diag)]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        is_preferred: Some(is_preferred),
        ..Default::default()
    }
}

/// A minimal `lsp_types::Diagnostic` echo (range + message) so the client can
/// associate the action with the diagnostic it resolves. The full diagnostic is
/// reconstructed by `fossil-lsp` when it publishes; here we only need the link.
fn lsp_diagnostic_stub(diag: &Diagnostic) -> LspDiagnostic {
    LspDiagnostic {
        message: diag.message.clone(),
        ..Default::default()
    }
}

/// Build the `file:` [`Uri`] for a source file from its interned path. The LSP
/// (06-08 `fossil-lsp`) keys its open-document table by the URI string; the
/// path stored on the `SourceFile` is that same string (or a bare filename in
/// tests), so we round-trip it through `Uri::from_str`, prepending the `file://`
/// scheme when the path is schemeless.
fn file_uri(db: &dyn fossil_base::Db, file: SourceFile) -> Uri {
    let path = file.path(db);
    let candidate = if path.contains("://") {
        path.clone()
    } else if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    };
    Uri::from_str(&candidate)
        .unwrap_or_else(|_| Uri::from_str("file:///unknown").expect("valid uri"))
}

/// Convert a `fossil_base::Span` (byte offsets) to a UTF-16 LSP [`Range`].
fn byte_span_to_range(index: &LineIndex, span: Span) -> Range {
    Range {
        start: to_position(index.position(span.start)),
        end: to_position(index.position(span.end)),
    }
}

/// Convert a UTF-16 LSP [`Range`] back to a byte-offset [`Span`] (for overlap
/// testing against the diagnostics' byte spans). A range whose endpoints fall
/// past EOF clamps to the file end.
fn range_to_byte_span(index: &LineIndex, range: Range) -> Span {
    let start = index.offset(from_position(range.start)).unwrap_or(u32::MAX);
    let end = index.offset(from_position(range.end)).unwrap_or(u32::MAX);
    Span::new(start.min(end), start.max(end))
}

const fn to_position(p: Utf16Position) -> Position {
    Position {
        line: p.line,
        character: p.character,
    }
}

const fn from_position(p: Position) -> Utf16Position {
    Utf16Position {
        line: p.line,
        character: p.character,
    }
}

/// Whether two byte spans intersect (touching endpoints count, so a zero-width
/// selection at a span boundary still surfaces the action).
const fn spans_overlap(a: Span, b: Span) -> bool {
    a.start <= b.end && b.start <= a.end
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use fossil_base::{NativeSystem, Severity, System};
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn System> = Arc::new(NativeSystem);
        fossil_base::FossilDb::new(system)
    }

    fn file(db: &fossil_base::FossilDb, src: &str) -> SourceFile {
        SourceFile::new(db, src.to_string(), "a.fossil".to_string())
    }

    /// A full-document range (covers everything) for tests.
    fn whole(db: &fossil_base::FossilDb, file: SourceFile) -> Range {
        let idx = line_index(db, file);
        let len = u32::try_from(file.text(db).len()).unwrap_or(u32::MAX);
        byte_span_to_range(&idx, Span::new(0, len))
    }

    #[test]
    fn did_you_mean_produces_a_replace_edit() {
        let db = db();
        // `.naem` typo at bytes 38..42 in the field-ref line.
        let src = "User : ex:Person from users\n    ex:n = .naem\n";
        let f = file(&db, src);
        let typo_start = u32::try_from(src.find("naem").unwrap()).unwrap();
        let span = Span::new(typo_start, typo_start + 4);
        let diag = Diagnostic::new(
            Severity::Error,
            "unknown column `naem` — did you mean `name`?",
            span,
        )
        .with_did_you_mean(span, "name");

        let actions = code_actions(&db, f, whole(&db, f), &[diag]);
        let a = actions
            .iter()
            .find(|a| a.title.contains("name"))
            .expect("a did-you-mean action must be offered");
        let edits = edits_of(a);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "name");
    }

    #[test]
    fn auto_import_inserts_well_known_prefix_line() {
        let db = db();
        // `xsd` used but undeclared.
        let src = "User : ex:Person from users\n    ex:age = xsd:integer\n";
        let f = file(&db, src);
        let diag = Diagnostic::new(
            Severity::Error,
            "undeclared prefix `xsd:` in IRI expression `xsd:integer`",
            Span::new(0, 5),
        );
        let actions = code_actions(&db, f, whole(&db, f), &[diag]);
        let a = actions
            .iter()
            .find(|a| a.title.contains("xsd"))
            .expect("an auto-import action must be offered");
        let edits = edits_of(a);
        assert_eq!(edits.len(), 1);
        assert!(
            edits[0]
                .new_text
                .contains("prefix xsd: <http://www.w3.org/2001/XMLSchema#>"),
            "auto-import must insert the canonical xsd decl; got {:?}",
            edits[0].new_text,
        );
        // Inserted at the very top of the file.
        assert_eq!(edits[0].range.start, Position::new(0, 0));
    }

    #[test]
    fn auto_import_unknown_prefix_uses_placeholder() {
        let db = db();
        let src = "User : ex:Person from users\n";
        let f = file(&db, src);
        let diag = Diagnostic::new(
            Severity::Error,
            "undeclared prefix `foo:` in IRI expression `foo:bar`",
            Span::new(0, 5),
        );
        let actions = code_actions(&db, f, whole(&db, f), &[diag]);
        let a = actions
            .iter()
            .find(|a| a.title.contains("foo"))
            .expect("an auto-import action for the unknown prefix must be offered");
        let edits = edits_of(a);
        assert!(
            edits[0].new_text.contains("prefix foo: <>"),
            "an unknown prefix must get a `<>` placeholder; got {:?}",
            edits[0].new_text,
        );
    }

    #[test]
    fn split_mapping_replaces_with_suggestion_source() {
        let db = db();
        let src = "Contact : ex:Contact from c\n    ex:email = .email\n";
        let f = file(&db, src);
        let snippet = "Contact1 : ex:Contact from c\n    ex:email = .email\n";
        let diag = Diagnostic::new(
            Severity::Error,
            "target shape ex:Contact uses ShEx OneOf",
            Span::new(0, u32::try_from(src.len()).unwrap()),
        )
        .with_suggestion_source(snippet);
        let actions = code_actions(&db, f, whole(&db, f), &[diag]);
        let a = actions
            .iter()
            .find(|a| a.title.contains("Split mapping"))
            .expect("a split-mapping action must be offered");
        let edits = edits_of(a);
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].new_text, snippet,
            "the split edit must use suggestion_source verbatim (not regenerated)",
        );
    }

    #[test]
    fn no_action_when_diagnostic_outside_range() {
        let db = db();
        let src = "User : ex:Person from users\n    ex:n = .naem\n";
        let f = file(&db, src);
        let span = Span::new(38, 42);
        let diag = Diagnostic::new(Severity::Error, "x", span).with_did_you_mean(span, "name");
        // A range over just line 0 (bytes 0..27) does not intersect the typo.
        let idx = line_index(&db, f);
        let r = byte_span_to_range(&idx, Span::new(0, 5));
        let actions = code_actions(&db, f, r, &[diag]);
        assert!(
            actions.is_empty(),
            "a diagnostic outside the requested range yields no action",
        );
    }

    /// Helper: extract the `TextEdit`s from the action's single-document
    /// `WorkspaceEdit`.
    fn edits_of(a: &CodeAction) -> Vec<TextEdit> {
        a.edit
            .as_ref()
            .and_then(|e| e.changes.as_ref())
            .and_then(|c| c.values().next())
            .cloned()
            .unwrap_or_default()
    }
}
