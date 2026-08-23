//! What an editor publishes for a file: which diagnostics it has, and what they
//! look like on the LSP wire.
//!
//! This is the one place both editor hosts ask. It was two places, and being two
//! places cost three defects in nine days:
//!
//! - `d.labels` was dropped on the floor by `fossil-lsp`'s `to_lsp_diagnostic`
//!   and by `fossil-wasm`'s `to_check_row`, so a report whose whole content is a
//!   RELATION between two places arrived as one squiggle with no second half.
//!   Both were found separately and fixed separately.
//! - The [`fossil_base::claimed`] guard — a `.shex` buffer is an INPUT and is not
//!   parsed as fossil — landed in `fossil-wasm`'s drain in `353228c`, and in
//!   `fossil-lsp`'s the following day in `470a13b`. The same defect, twice, in
//!   two files, a day apart. Between the two commits an editor put twenty-one
//!   squiggles down the length of the user's `ShEx` and the browser did not.
//! - The worker's `publishDiagnostics` carried `related` where LSP says
//!   `relatedInformation`, with a flat `uri`/`range` where LSP says a nested
//!   `location`, because it republished the playground's JS row shape verbatim.
//!   A client reading the spec found nothing there.
//!
//! `crates/fossil-lsp/tests/transport_parity.rs` is the guard that would have
//! caught all three: it drives both transports over the same buffer and compares
//! the JSON. This module is what makes the comparison pass by construction
//! rather than by vigilance.
//!
//! # The two questions are separate on purpose
//!
//! [`diagnostics()`] answers WHICH — the drain, plus the rule about which open
//! files are programs. [`lsp_diagnostics()`] answers WHAT THEY LOOK LIKE.
//! `fossil-wasm` needs the first on its own for the playground's `check()`
//! array, whose row shape is not LSP and is keyed by whatever path the host
//! opened the buffer under; see [`crate::related`] for why turning one of those
//! into a URI is not always possible.

use fossil_base::{Db, Diagnostic, Severity, SourceFile, Span};
use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Range,
    Uri,
};

use crate::line_index::LineIndex;
use crate::position::{line_index, offset_to_lsp_position};
use crate::related::related_locations;

/// Every diagnostic `file` produces, from the one implementation of that
/// question — [`fossil_mir::program_diagnostics`], which `fossil-engine` also
/// calls.
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
/// output contract, and in the playground it is the ONLY way to hand the
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
        .map(|d| lsp_diagnostic(db, file, &index, d))
        .collect()
}

/// One [`fossil_base::Diagnostic`] as LSP sees it.
///
/// Byte spans become UTF-16 ranges; `suggestion_source` is folded into the
/// message as a `help:` line. The structured carriers (`did_you_mean`,
/// `suggestion_source`) have no place on the wire and are not sent — a host that
/// needs them for a code action re-drains [`diagnostics()`], which is what both
/// do.
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
    let related: Vec<DiagnosticRelatedInformation> = related_locations(db, file, d)
        .into_iter()
        .filter_map(|r| {
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: file_uri(r.file.path(db))?,
                    range: span_to_range(&line_index(db, r.file), r.span),
                },
                message: r.text,
            })
        })
        .collect();
    LspDiagnostic {
        range: span_to_range(index, d.span),
        severity: Some(severity(d.severity)),
        message: d.suggestion_source.as_ref().map_or_else(
            || d.message.clone(),
            |s| format!("{}\nhelp: {s}", d.message),
        ),
        related_information: (!related.is_empty()).then_some(related),
        ..LspDiagnostic::default()
    }
}

/// A registry key as an LSP `Uri`.
///
/// The keys are the program's own path with the document joined onto it
/// ([`fossil_hir::documents::registry_key`]), so in an editor they are already
/// `file://` URIs and this is the identity. A key that is a bare path — what a
/// test, or a playground host that opened a buffer by name, produces — gets
/// `file://` in front of it; `None` for a key that is neither.
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
        Uri::from_str(key).ok()
    } else if let Some(rest) = key.strip_prefix('/') {
        Uri::from_str(&format!("file:///{rest}")).ok()
    } else {
        Uri::from_str(&format!("file://{key}")).ok()
    }
}

/// A byte-offset [`Span`] as a UTF-16 LSP [`Range`].
#[must_use]
pub fn span_to_range(index: &LineIndex, span: Span) -> Range {
    Range {
        start: position(index, span.start),
        end: position(index, span.end),
    }
}

/// A byte-offset range (what the feature functions return) as a UTF-16 LSP
/// [`Range`].
#[must_use]
pub fn byte_range_to_range(index: &LineIndex, range: std::ops::Range<u32>) -> Range {
    Range {
        start: position(index, range.start),
        end: position(index, range.end),
    }
}

fn position(index: &LineIndex, offset: u32) -> lsp_types::Position {
    let p = offset_to_lsp_position(index, offset);
    lsp_types::Position {
        line: p.line,
        character: p.character,
    }
}

const fn severity(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
    }
}
