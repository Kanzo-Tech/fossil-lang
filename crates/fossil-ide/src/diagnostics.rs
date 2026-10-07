//! What an editor publishes for a file: which diagnostics it has, and what they
//! look like on the LSP wire.
//!
//! This is the one place both editor hosts ask. It was two places, and being two
//! places cost three defects in nine days:
//!
//! - `d.labels` was dropped on the floor by `fossil-lsp`'s `to_lsp_diagnostic`
//!   and by `fossil-wasm`'s `to_diagnostic`, so a report whose whole content is a
//!   RELATION between two places arrived as one squiggle with no second half.
//!   Both were found separately and fixed separately.
//! - The [`fossil_base::claimed`] guard — a `.shex` buffer is an INPUT and is not
//!   parsed as fossil — landed in `fossil-wasm`'s drain in `353228c`, and in
//!   `fossil-lsp`'s the following day in `470a13b`. The same defect, twice, in
//!   two files, a day apart. Between the two commits an editor put twenty-one
//!   squiggles down the length of the user's `ShEx` and the browser did not.
//! - The worker's `publishDiagnostics` carried `related` where LSP says
//!   `relatedInformation`, with a flat `uri`/`range` where LSP says a nested
//!   `location`, because it republished the wasm workspace's JS row shape verbatim.
//!   A client reading the spec found nothing there.
//!
//! The worker transport is deleted since; this module is what made the two
//! agree by construction rather than by vigilance.
//!
//! # The two questions are separate on purpose
//!
//! [`diagnostics()`] answers WHICH — the drain, plus the rule about which open
//! files are programs. [`lsp_diagnostics()`] answers WHAT THEY LOOK LIKE.
//! `fossil-wasm` needs the first on its own for the workspace's `check()`
//! array, whose row shape is not LSP and is keyed by whatever path the host
//! opened the buffer under; see [`crate::related`] for why turning one of those
//! into a URI is not always possible.

use fossil_base::{Db, Diagnostic, SourceFile};
use fossil_graph_schema::Severity;
use lsp_types::{
    CodeDescription, Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    Location, NumberOrString, Range, Uri,
};

use crate::position::{LineIndex, line_index, range};
use crate::related::related_locations;

/// Every diagnostic `file` produces, from the one implementation of that
/// question — [`fossil_mir::program_diagnostics`], which the conformance
/// harness also calls.
///
/// Each host used to run a per-mapping loop of its own instead, and what the
/// editors did not show for as long as that was true: a file the parser
/// recovered no mapping from published NO diagnostics at all (the parse errors
/// were present and unreachable), a top-level binding's provider errors
/// vanished, two mappings minting two identities for one type was never checked,
/// and one top-level mistake was published once per mapping.
///
/// # A file the catalogue reads is not drained as a program
///
/// Opening the `.shex` a program names is the ordinary way to look at your own
/// output contract, and in the wasm workspace it is the ONLY way to hand the
/// compiler one. Without the guard below that ran the fossil parser over the
/// document and attributed every complaint to it — twenty-one diagnostics
/// measured over the wire for a `ShExJ` document with nothing wrong with it.
///
/// The question «which open files are programs» is the provider catalogue's: a
/// row declares the extensions it accepts, [`fossil_base::claimed`] asks all of
/// them at once, and a URI some row READS is an input to a program rather than a
/// program. Nothing about fossil's syntax is decided here. Both hosts install
/// `fossil_descriptors_output::PROVIDERS`, and a buffer's registry path is its
/// whole URI in both, which `Provider::accepts` reads exactly as it reads a
/// path.
///
/// Three things this deliberately does not do.
///
/// It does not consult a **program** extension. `.fossil` is a convention, a URI
/// with no extension is claimed by nobody, and the default is to check — so an
/// unrecognised file falls back to the old behaviour and never to silence.
///
/// It does not deregister the document. A host puts an opened `.shex` in the
/// file registry under its own URI precisely so the OPEN COPY is what every
/// program naming it reads: the buffer is still decoded, so a program is still
/// checked against the document it names and a broken `ShEx` is still reported
/// *on the program*. What has no home is a document nobody names — nothing
/// checks it, because there is nothing to check it against.
///
/// And it does not stop at what is published. Both hosts' `codeAction` handlers
/// re-drain through this function, so no quick-fix is offered inside a shape
/// document either. That is right and not a side effect: every action
/// [`crate::code_actions`] can build is keyed off a fossil diagnostic and edits
/// fossil source, so inside a `.shex` it would be a lightbulb rewriting the
/// user's `ShEx` into fossil.
#[must_use]
pub fn diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    if fossil_base::claimed(fossil_base::installed(db), file.path(db)) {
        return Vec::new();
    }
    fossil_mir::program_diagnostics(db, file)
}

/// [`diagnostics()`], rendered for `textDocument/publishDiagnostics`.
///
/// The whole payload of the notification both transports send. Nothing above
/// this adds a field or renames one.
#[must_use]
pub fn lsp_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<LspDiagnostic> {
    let index = line_index(db, file);
    diagnostics(db, file)
        .iter()
        .map(|d| lsp_diagnostic(db, file, index, d))
        .collect()
}

/// One [`fossil_base::Diagnostic`] as LSP sees it.
///
/// Byte spans become UTF-16 ranges. `message` is the problem rendered and
/// nothing else; `code` is its code, `codeDescription.href` the code's page
/// ([`fossil_graph_schema::Problem::help_url`]), and `data` the rest of it —
/// [`DiagnosticData`]. A code action still reads `did_you_mean` and
/// `suggestion_source` off the re-drained [`diagnostics()`], so the copy in
/// `data` is for a client that has no drain.
///
/// # The labels are `relatedInformation`, and it takes a URI per entry
///
/// A label pointing into the `.shex` the program named lands on THAT file, which
/// is what makes the two-file report an editor feature and not a terminal one.
/// [`related_locations`] answers which file each label is in;
/// [`file_uri`] turns that answer into the one thing LSP will accept.
///
/// A `LineIndex` per file and not the program's: a UTF-16 column is a fact about
/// the text the range is in, and reusing the program's index over a document's
/// bytes puts the entry on a plausible wrong line. `line_index` is memoised per
/// file, so the program's own labels cost nothing extra.
#[must_use]
pub fn lsp_diagnostic(
    db: &dyn Db,
    file: SourceFile,
    index: &LineIndex,
    d: &Diagnostic,
) -> LspDiagnostic {
    use std::str::FromStr as _;
    let related: Vec<DiagnosticRelatedInformation> = related_locations(db, file, d)
        .into_iter()
        .filter_map(|r| {
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: file_uri(r.file.path(db))?,
                    range: range(line_index(db, r.file), r.span),
                },
                message: r.text,
            })
        })
        .collect();
    LspDiagnostic {
        range: range(index, d.span),
        severity: Some(severity(d.severity)),
        code: Some(NumberOrString::String(d.problem.code().into())),
        code_description: Uri::from_str(&d.problem.help_url())
            .ok()
            .map(|href| CodeDescription { href }),
        message: d.message(),
        related_information: (!related.is_empty()).then_some(related),
        data: serde_json::to_value(DiagnosticData::of(index, d)).ok(),
        ..LspDiagnostic::default()
    }
}

/// What a diagnostic says beside its code and its message: LSP's `data`, and
/// the same fields on `fossil-wasm`'s `Diagnostic`.
///
/// Serialized camelCase, every optional absent rather than `null`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticData {
    /// Fixed per code — [`fossil_graph_schema::Problem::title`].
    pub title: &'static str,
    /// The problem's `data`: the values its message interpolates, typed per
    /// code by `ProblemData` in `@fossil-lang/types`.
    pub data: serde_json::Value,
    /// What to do about it, in prose — [`Diagnostic::help`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// A replacement for a misspelt name — [`Diagnostic::did_you_mean`], its
    /// span as a UTF-16 range in the diagnostic's own file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did_you_mean: Option<Replacement>,
    /// Fossil source that repairs it — [`Diagnostic::suggestion_source`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Replace the text at `range` with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct Replacement {
    #[schemars(with = "crate::wire::RangeSchema")]
    pub range: Range,
    pub replacement: String,
}

impl DiagnosticData {
    /// `d`'s structured half, its ranges measured against `index` — the line
    /// index of the file `d` is reported in.
    #[must_use]
    pub fn of(index: &LineIndex, d: &Diagnostic) -> Self {
        let data = serde_json::to_value(&d.problem)
            .ok()
            .and_then(|mut wire| wire.get_mut("data").map(serde_json::Value::take))
            .unwrap_or_default();
        Self {
            title: d.problem.title(),
            data,
            help: d.help.clone(),
            did_you_mean: d.did_you_mean.as_ref().map(|m| Replacement {
                range: range(index, m.wrong_span),
                replacement: m.replacement.clone(),
            }),
            suggestion: d.suggestion_source.clone(),
        }
    }
}

/// A registry key as an LSP `Uri`.
///
/// The keys are the program's own path with the document joined onto it
/// ([`fossil_hir::documents::registry_key`]), so in an editor they are already
/// `file://` URIs and this is the identity. A key that is a bare path — what a
/// test, or a browser host that opened a buffer by name, produces — is a path
/// on the local host, `file:///<path>` (RFC 8089: `file://x` would make `x` a
/// host); `None` for a key that is neither.
///
/// **This was three functions across the two hosts and they did not agree.**
/// `fossil-lsp` had one for goto-def (`UriExt::from_str_maybe`) and a second for
/// related information (`path_to_uri`, which routed through a `file://`-only
/// helper and silently dropped every other scheme), and `fossil-wasm` used the
/// first for goto-def and, for related information, no conversion at all — it
/// put the raw key on the wire. For the two key shapes an editor actually
/// produces all three agreed, which is why the disagreement was never seen.
#[must_use]
pub fn file_uri(key: &str) -> Option<Uri> {
    use std::str::FromStr as _;
    if key.contains("://") {
        return Uri::from_str(key).ok();
    }
    let mut url = url::Url::parse("file:///").ok()?;
    url.set_path(key);
    Uri::from_str(url.as_str()).ok()
}

const fn severity(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
    }
}

#[cfg(test)]
mod tests {
    use super::file_uri;

    /// A bare path is a path on the local host, absolute or not: `file://x`
    /// would name `x` as a host, and the code-action edits and the diagnostics
    /// once disagreed on exactly this.
    #[test]
    fn a_bare_key_is_a_local_path() {
        let uri = |k| file_uri(k).map(|u| u.as_str().to_owned());
        assert_eq!(uri("main.fossil").as_deref(), Some("file:///main.fossil"));
        assert_eq!(
            uri("/w/main.fossil").as_deref(),
            Some("file:///w/main.fossil")
        );
        assert_eq!(uri("untitled://x").as_deref(), Some("untitled://x"));
    }

    /// The path is percent-encoded the way the URL Standard encodes one: a space
    /// in a workspace path is `%20`, not a URI that fails to parse.
    #[test]
    fn a_bare_key_is_percent_encoded() {
        let uri = |k| file_uri(k).map(|u| u.as_str().to_owned());
        assert_eq!(
            uri("/my work/a b.fossil").as_deref(),
            Some("file:///my%20work/a%20b.fossil")
        );
    }
}
