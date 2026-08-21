//! Where each of a diagnostic's labels points, resolved for an editor host.
//!
//! A `fossil_base::Diagnostic` can underline several places and they are not
//! all in the same file: `` `total` expects Float `` blames a line of the
//! program, and the other half of the sentence is a line of the `.shex` the
//! program named (`fossil_base::SpanLabel::document`). LSP has the concept
//! exactly — `DiagnosticRelatedInformation { location: { uri, range }, message }`
//! — and it takes a URI per entry, which a label does not carry.
//!
//! # The hosts had NO labels at all, not just no document ones
//!
//! `fossil-lsp`'s `to_lsp_diagnostic` converted the span, the severity and the
//! message and dropped `labels` on the floor; `fossil-wasm`'s `to_check_row`
//! did the same. So a report whose whole content is a RELATION between two
//! places — «`Users` and `Imported` mint two identities for Person», which
//! names both mappings and underlines both `@subject` lines — reached an editor
//! as one squiggle with no second half, and had since the day labels existed.
//! `fossil check` rendered it in full the whole time, which is why nothing
//! caught it.
//!
//! # One answer, two renderings
//!
//! This resolves the QUESTION — which file and which range each label is in —
//! and stops there. `fossil-lsp` turns the answer into
//! `DiagnosticRelatedInformation`, `fossil-wasm` into its JS-facing row shape,
//! and neither re-derives a URI. That is the split this workspace keeps
//! arriving at: `documents_named` and `registry_key` are one function each for
//! the same reason, after the editor's hand-rolled copy of the second silently
//! disagreed with the checker's on a `://` scheme.

use fossil_base::{Db, Diagnostic, SourceFile, Span, file_at};
use fossil_hir::documents::registry_key;

/// One label, with the file it is in resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    /// The file this range is in — the program itself for a label in the
    /// program, the registered document otherwise.
    ///
    /// The `SourceFile` and not a path, because a host needs both: its `path`
    /// for the URI, and the file itself for [`crate::line_index()`], which is
    /// memoised per file. Building a second index over the same bytes would be
    /// a second answer to where line 4 starts.
    ///
    /// Not a URI either: turning a path into one is the host's business and the
    /// two do it differently — a `file://` string on disk, a bare name in the
    /// browser, where there is none.
    pub file: SourceFile,
    /// The range, file-absolute in that file's text.
    pub span: Span,
    /// What to say about it.
    pub text: String,
}

/// Resolve every label of `d` against the file it belongs to.
///
/// Labels are already file-absolute by the time a host sees them —
/// `fossil_hir::spans::rebase_to_file` shifts each part by ITS OWN frame, and a
/// document label is `FileAbsolute` because a document has no mappings to be
/// relative to.
///
/// **A label naming a document that is not registered is dropped.** That is a
/// document the host could not read, which is the same thing the checker saw,
/// and a range resolved against the wrong text is worse than a missing entry.
#[must_use]
pub fn related_locations(db: &dyn Db, file: SourceFile, d: &Diagnostic) -> Vec<Related> {
    d.labels
        .iter()
        .filter_map(|label| {
            let in_file = match label.document.as_deref() {
                None => file,
                // Registered, or there is no text to resolve against.
                Some(document) => file_at(db, &registry_key(db, file, document))?,
            };
            Some(Related {
                file: in_file,
                span: label.span,
                text: label.text.clone(),
            })
        })
        .collect()
}
