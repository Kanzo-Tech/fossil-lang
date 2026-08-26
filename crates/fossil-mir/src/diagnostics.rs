//! Every diagnostic a program produces, drained once, by whoever asks.
//!
//! # Why this is a crate function and not three copies of a loop
//!
//! «What is wrong with this program» was written three times: once in
//! `fossil_cli::check`, once in `fossil-lsp`'s `diagnostics_for`, once in
//! `fossil-wasm`'s `diagnostics_for_file`. The two editor copies were
//! byte-identical to each other, comments included, and both were the SHORT
//! version — a per-mapping drain and nothing else. The engine's was the long
//! one, and every line it had that they did not was a line added to close a
//! measured hole:
//!
//! - **A file with no mapping reported nothing.** Salsa accumulates over a
//!   query's dependency subtree, and `parse` is only in the subtree of a
//!   mapping — so a file broken badly enough that the parser recovered no
//!   mapping had its parse errors present and unreachable. The editor and the
//!   browser show a clean document for a file that does not parse.
//! - **A top-level BINDING's diagnostics were lost with it.** `lower_to_hir` is
//!   where `check_provider` / `check_schema_arg` run, and it is not in
//!   `def_map`'s subtree. Same file, same silence.
//! - **Identity was never checked at all.** Two mappings that mint two
//!   `@subject` templates for one type is an ERROR, raised by
//!   [`fossil_hir::identity::check_identities`], which is file-keyed and which
//!   a per-mapping loop never reaches.
//! - **One mistake was printed once per mapping.** The file-level queries are
//!   in every mapping's subtree, so a program with ten mappings reported one
//!   top-level mistake ten times.
//!
//! None of those is an editor concern or a command-line concern; all four are
//! facts about how the compiler's queries accumulate. Three hosts asking one
//! question got two different answers, and the answer the two editors got was
//! the wrong one — which is the failure mode `/docs/design/three-hosts` names:
//! *a verb that exists only behind the binary is not a smaller surface; it is a
//! surface with no embedding.*
//!
//! # Why it lives in `fossil-mir`
//!
//! Forced, not chosen. The drain has to sit above every query that emits: the
//! parser (`fossil-syntax`), `def_map` / `lower_to_hir` / `check_identities`
//! (`fossil-hir`) and [`crate::lower_to_mir_pg`] (here). `fossil-mir` is the
//! lowest crate above all of them, it compiles to `wasm32`, and all three hosts
//! already depend on it.
//!
//! `fossil-ide` is the crate this would otherwise belong to — rust-analyzer
//! puts diagnostics in its `ide` façade, and both editor hosts already share
//! `fossil-ide`. It is ruled out by a decision already taken: `fossil-cli`
//! may not depend on the editor surface, which is the arrangement
//! `fossil-lineage` exists to prevent and whose module docs say so. `fossil-ide`
//! also does not depend on `fossil-mir`, so it could not reach the lowering
//! drain without acquiring that edge too.

use std::collections::HashSet;

use fossil_base::{Db, Diagnostic, Severity, SourceFile, Span, SpanFrame};

/// Every diagnostic `file` produces, spans file-absolute, no duplicates.
///
/// Drains from [`crate::lower_to_mir_pg`] rather than
/// `fossil_hir::check::typecheck_mapping`: Salsa accumulators are transitive
/// and lowering calls the typechecker, so this yields the typecheck diagnostics
/// PLUS the lowering ones without duplicating either. Draining only the
/// typechecker made `check` report `ok` for a program `run` then refused — a
/// mapping reading `from` a derived binding, whose source cannot be resolved.
/// What a host reports must not pass what `run` rejects.
///
/// This forces the queries it drains. A caller does not need to have run the
/// compiler first, and a caller that has already run it pays nothing: the
/// results are memoised.
#[must_use]
pub fn program_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut diagnostics = file_level(db, file);
    diagnostics.extend(per_mapping(db, file));
    dedup(&mut diagnostics);
    diagnostics
}

/// The FILE-keyed queries, drained unconditionally.
///
/// Unconditionally, and not `if mappings.is_empty()`, which is where half of
/// the no-mapping bug lived: `lower_to_hir` checks top-level BINDINGS and is
/// not in `def_map`'s subtree, so a file WITH mappings lost every provider
/// diagnostic its bindings produced. Both are file-keyed, so draining them
/// always is correct and the duplicates it creates are removed by [`dedup`].
///
/// Their spans are file-absolute — the parser and `lower_to_hir` both measure
/// against the file — which is why nothing is rebased here.
fn file_level(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> =
        fossil_hir::def_map::def_map::accumulated::<Diagnostic>(db, file)
            .into_iter()
            .cloned()
            .collect();
    out.extend(
        fossil_hir::lower::lower_to_hir::accumulated::<Diagnostic>(db, file)
            .into_iter()
            .cloned(),
    );

    // One identity per type: every mapping that produces `T` declares the same
    // `@subject`, and two that disagree are an ERROR — never a warning —
    // naming both mappings and both templates, because a warning about identity
    // gets ignored and the result is two entities where there was one.
    //
    // It is a third file-level drain and not a fourth per-mapping one because
    // uniqueness is a fact about the FILE: `body::check_identity` is keyed by
    // `MappingLoc` and by construction cannot see a second mapping. Its own
    // spans are file-absolute — it rebased both of them itself, being the only
    // party that holds two mappings at once.
    //
    // FILTERED to the file-absolute ones, and that is not a nicety. Unlike the
    // two drains above, this query sits BELOW `body`: salsa accumulates over the
    // whole dependency subtree, so draining it unfiltered also yields every
    // mapping-relative diagnostic every body produced — raw, while
    // [`per_mapping`] yields the same ones REBASED. Two spans, so [`dedup`]
    // cannot see them as one, and the raw copy points at whatever sits at that
    // offset from the start of the file. A mapping-relative diagnostic has an
    // owner and this drain is not it.
    let _ = fossil_hir::identity::check_identities(db, file);
    out.extend(
        fossil_hir::identity::check_identities::accumulated::<Diagnostic>(db, file)
            .into_iter()
            .filter(|d| d.frame == SpanFrame::FileAbsolute)
            .cloned(),
    );
    out
}

/// Each mapping's own diagnostics, rebased onto the file.
fn per_mapping(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mut out = Vec::new();
    for mapping in def_map.mappings(db) {
        let _ = crate::lower_to_mir_pg(db, *mapping);
        let diags = crate::lower_to_mir_pg::accumulated::<Diagnostic>(db, *mapping);
        // Spans come out mapping-relative; every host renders against the file.
        out.extend(fossil_hir::spans::rebase_to_file(
            db,
            *mapping,
            diags.into_iter().cloned(),
        ));
    }
    out
}

/// Drop repeats, keeping the first of each.
///
/// Every mapping's dependency subtree contains the file-keyed queries above, so
/// a diagnostic about a top-level binding — `type { P } := io.csv("users.csv")`
/// — comes out once per mapping. It is not a per-mapping fact and there is no
/// mapping to attribute it to.
///
/// The key is `(severity, message, span)`, and each part is load-bearing. Two
/// diagnostics with one message at two spans are two mistakes and both survive
/// — which is why this is not a `message`-only dedup. Two with one message at
/// ONE span are one statement about one range of bytes, and printing it twice
/// is noise by construction, whichever query emitted it.
///
/// It runs over the whole list rather than only the file-level drains because
/// the per-mapping path is where the duplicates actually arrive: they are the
/// file-level ones, carried along by `lower_to_mir_pg::accumulated`, and there
/// is nothing at that point marking which is which.
fn dedup(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen: HashSet<(Severity, String, Span)> = HashSet::new();
    diagnostics.retain(|d| seen.insert((d.severity, d.message.clone(), d.span)));
}
