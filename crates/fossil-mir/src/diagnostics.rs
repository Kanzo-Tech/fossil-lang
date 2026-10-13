//! Every diagnostic a program produces, collected once, by whoever asks.
//!
//! # Why this is a crate function and not three copies of a loop
//!
//! «What is wrong with this program» is one question, and each query that can
//! find a mistake answers only its part of it, in its own output: the parse,
//! the lowering, a mapping's body, its check, its lowering to MIR, and the
//! file's identities. Each line below closes a measured hole a host's own loop
//! left open:
//!
//! - **A file with no mapping reported nothing.** A per-mapping loop never
//!   reaches `parse`, so a file broken badly enough that the parser recovered
//!   no mapping showed a clean document.
//! - **A top-level BINDING's diagnostics were lost.** `lower_to_hir` is where
//!   `check_provider` / `check_schema_arg` run, and it is keyed by the file.
//! - **Identity was never checked at all.** Two mappings that mint two
//!   `@subject` templates for one type is an ERROR, raised by
//!   [`fossil_hir::identity::check_identities`], which is file-keyed and which
//!   a per-mapping loop never reaches.
//! - **One mistake was printed once per mapping.** A check and a lowering can
//!   both refuse the same source, at the same span; [`dedup`] keeps the first.
//!
//! None of those is an editor concern or a command-line concern; all four are
//! facts about where the compiler's queries put what they find. Three hosts asking one
//! question got two different answers, and the answer the two editors got was
//! the wrong one — which is the failure mode `/docs/design/three-hosts` names:
//! *a verb that exists only behind the binary is not a smaller surface; it is a
//! surface with no embedding.*
//!
//! # Why it lives in `fossil-mir`
//!
//! Forced, not chosen. The collector has to sit above every query that reports: the
//! parser (`fossil-syntax`), `def_map` / `lower_to_hir` / `check_identities`
//! (`fossil-hir`) and [`crate::lower_to_mir_pg`] (here). `fossil-mir` is the
//! lowest crate above all of them, it compiles to `wasm32`, and all three hosts
//! already depend on it.
//!
//! `fossil-ide` is the crate this would otherwise belong to — rust-analyzer
//! puts diagnostics in its `ide` façade, and both editor hosts already share
//! `fossil-ide`. It was ruled out while a native CLI existed, which
//! could not depend on the editor surface; `fossil-ide` also does not depend on `fossil-mir`, so it could not reach the lowering
//! without acquiring that edge too.

use std::collections::HashSet;

use fossil_base::{Db, Diagnostic, SourceFile, SpanFrame};
use fossil_graph_schema::{Problem, Severity, Span};

/// Every diagnostic `file` produces, spans file-absolute, no duplicates.
///
/// Reads [`crate::lower_to_mir_pg`] beside `fossil_hir::check::typecheck_mapping`:
/// draining only the checker made `check` report `ok` for a program `run` then
/// refused — a mapping reading `from` a derived binding, whose source cannot be
/// resolved. What a host reports must not pass what `run` rejects.
///
/// This forces the queries it reads. A caller does not need to have run the
/// compiler first, and a caller that has already run it pays nothing: the
/// results are memoised.
#[must_use]
pub fn program_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut diagnostics = file_level(db, file);
    diagnostics.extend(per_mapping(db, file));
    dedup(&mut diagnostics);
    diagnostics
}

/// The FILE-keyed queries, read unconditionally.
///
/// Unconditionally, and not `if mappings.is_empty()`, which is where half of
/// the no-mapping bug lived: a file WITH mappings still has top-level bindings,
/// and `lower_to_hir` is where their providers are checked.
///
/// Their spans are file-absolute — the parser and `lower_to_hir` both measure
/// against the file — which is why nothing is rebased here.
fn file_level(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = fossil_syntax::parse(db, file).diagnostics(db).clone();
    out.extend(
        fossil_hir::lower::lower_to_hir(db, file)
            .diagnostics(db)
            .iter()
            .cloned(),
    );

    // One identity per type: every mapping that produces `T` declares the same
    // `@subject`, and two that disagree are an ERROR — never a warning —
    // naming both mappings and both templates, because a warning about identity
    // gets ignored and the result is two entities where there was one.
    //
    // It is file-level and not per-mapping because uniqueness is a fact about
    // the FILE: `body::check_identity` is keyed by `MappingLoc` and by
    // construction cannot see a second mapping. Its spans are file-absolute —
    // it rebased both of them itself, being the only party that holds two
    // mappings at once — and the filter keeps a mapping-relative one, which
    // would have an owner elsewhere, from being shown unrebased.
    out.extend(
        fossil_hir::identity::check_identities(db, file)
            .iter()
            .filter(|d| d.frame == SpanFrame::FileAbsolute)
            .cloned(),
    );
    out
}

/// Each mapping's own diagnostics, rebased onto the file: its body, its check
/// and its lowering to MIR.
///
/// A mapping whose header `lower_to_hir` declined is not checked: the check
/// would run against no shape and no row, and every complaint it made would be
/// a cascade of the one the header already got. Its body still is — it reads
/// the CST directly and needs nothing the header failed to give, and without it
/// a header one token wrong would lose every report about the lines under it.
fn per_mapping(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let hir = fossil_hir::lower::lower_to_hir(db, file);
    let mut out = Vec::new();
    for (index, mapping) in def_map.mappings(db).iter().enumerate() {
        let mut own: Vec<Diagnostic> = crate::lower_to_mir_pg(db, *mapping).diagnostics(db).clone();
        own.extend(
            fossil_hir::body::body(db, *mapping)
                .diagnostics(db)
                .iter()
                .cloned(),
        );
        if hir.mapping(db, index).is_some() {
            own.extend(
                fossil_hir::check::typecheck_mapping(db, *mapping)
                    .diagnostics(db)
                    .iter()
                    .cloned(),
            );
        }
        // Spans come out mapping-relative; every host renders against the file.
        out.extend(fossil_hir::spans::rebase_to_file(db, *mapping, own));
    }
    out
}

/// Drop repeats, keeping the first of each.
///
/// A check and a lowering read the same source binding, so a refusal of it —
/// an unresolvable row, say — can come out of both, once per mapping that
/// reads it. It is one fact and there is one place to print it.
///
/// The key is `(severity, problem, span)`, and each part is load-bearing. Two
/// diagnostics with one problem at two spans are two mistakes and both survive
/// — which is why this is not a `problem`-only dedup. Two with one problem at
/// ONE span are one statement about one range of bytes, and printing it twice
/// is noise by construction, whichever query emitted it.
fn dedup(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen: HashSet<(Severity, Problem, Span)> = HashSet::new();
    diagnostics.retain(|d| seen.insert((d.severity, d.problem.clone(), d.span)));
}
