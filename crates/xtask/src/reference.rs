//! The catalogue, projected onto the reference page — `apps/docs/content/generated/stdlib.mdx`.
//!
//! # Why this exists
//!
//! The catalogue is DATA and a compiler reads it. `crate::catalogue` parses
//! `catalogue.bnf`; this turns the same parse into the page's tables.
//!
//! `apps/docs/content/docs/book/stdlib.mdx` was that table written a second
//! time, by hand, in Markdown — and the second copy had drifted, which is what
//! a second copy does. Measured on 2026-08-23, before this file existed:
//!
//! - 51 rows in the registry, **every one of them on the page**;
//! - 58 distinct rows on the page, so **seven named nothing the checker knows**;
//! - of those seven, `io.rdf`, `io.shex` and `io.shacl` were real language
//!   surface the registry did not carry; `seq.filter` was renamed to
//!   `seq.where`; `str.normalize_unicode` was deleted from the language
//!   outright; and `io.sql` and `io.http` occur nowhere in the repository at
//!   all — the page is their only existence.
//!
//! The seven are the argument for generating this. A reader could not have told
//! them apart from the 51, because the page rendered all 58 identically.
//!
//! **Twelve of the 51 that did exist described a call the checker rejects** —
//! wrong arity, or a named argument naming no parameter of the row: `dir` on
//! `core.lang`, `digits` on `math.round`, `end` on `str.slice`, `delimiter` and
//! `header` on `io.csv`, `from`/`to` on `str.replace` (`needle`/`replacement`),
//! `sep` on `str.split` (`separator`), `by`/`key`/`desc` on `seq.distinct`,
//! `seq.group_by` and `seq.sort`, which have no such parameters, and
//! `str.concat` written variadic where it takes two. That is why [`signature`]
//! prints every parameter WITH ITS NAME: the other 39 rows differed only in
//! notation, and a page that cannot be told apart from one that is wrong is not
//! a reference.
//!
//! # One source, at last
//!
//! This module used to read TWO: `catalogue.bnf` for the `io` table and
//! `fossil_hir::stdlib`'s `FunctionRegistry` for every other section, because
//! only the `io.` half of the catalogue had become a file. Fifteen lines of
//! module doc existed to explain which datum came from where.
//!
//! Both halves are in the file now, so the special case is gone and so is
//! `xtask`'s dependency on `fossil-hir` — which mattered for a reason beyond
//! tidiness. `cargo xtask catalogue` GENERATES `fossil-hir`'s stdlib table; a
//! generator that must link the crate it writes cannot be run to repair a bad
//! emit, because the bad emit is what stops the crate compiling. `fossil-hir`
//! is a DEV-dependency now: the tests can still hold the generated table
//! against the registry it becomes, and the binary that fixes it links none of
//! it.
//!
//! # What this does NOT generate, deliberately
//!
//! The ARGUMENT — the same line `crate::catalogue` draws, for the same reason.
//! `book/stdlib.mdx` keeps every sentence: what `slug` does to a string, which
//! four `math` rows go inside `group_by`, why `io`'s constructors take nothing
//! on the left. A generator that tried to carry prose would make the catalogue
//! a documentation format with a type checker attached to it.
//!
//! What crosses the line is a NAME, a SIGNATURE, an EXTENSION and a LOWERING.
//! Each is a field of a row; none of them is a sentence.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::catalogue::{Catalogue, FnRow, Lowering, Reads, Row, Signature, is_namespace_head};

/// Where the emitted partial goes, repo-relative.
///
/// Outside `content/docs/`, which is the whole of why it is here and not beside
/// the page: `defineDocs({ dir: "content/docs" })` routes every `.mdx` under
/// that directory, so a partial living there would be a page of the site.
/// `remarkInclude` resolves `<include>` against the INCLUDING file's directory,
/// so `book/stdlib.mdx` reaches it as `../../generated/stdlib.mdx`.
pub const PARTIAL: &str = "apps/docs/content/generated/stdlib.mdx";

/// The receiver head of a dotted catalogue name — `str` of `str.trim`.
fn head_of(name: &str) -> &str {
    name.split_once('.').map_or(name, |(head, _)| head)
}

/// `(text: String, start: Integer) -> String`.
///
/// Every parameter is printed WITH ITS NAME, and that is the part the hand
/// written page could not be trusted on. A name here is not decoration: it is
/// what a named argument writes to reach the position (`grammar.bnf, NamedArg`),
/// so a page that called `str.replace`'s parameters `from` and `to` — as this
/// one did — documented a call the checker rejects, the row spelling them
/// `needle` and `replacement`.
///
/// The notation carries the two things a name and a type cannot: `…` for a
/// position that repeats and `name =` for one written with its name. Both were
/// unprintable while arity and named-ness did not exist, and their absence is
/// what let this page write `sort(by)`, `distinct(key)` and `group_by(desc)` —
/// three parameters no row had.
#[must_use]
pub fn signature(sig: &Signature) -> String {
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| {
            // `on = Predicate`, not `on: Predicate`. The separator is the one
            // the call site writes — and the one `catalogue.bnf` writes — so a
            // reader copying the signature copies a program that parses.
            let sep = if p.named { " =" } else { ":" };
            let repeat = p.arity.page_suffix();
            // An aggregation's name is the PROGRAM's, so printing the
            // position's own label with an `=` in front of it would document
            // `aggregations = …` as a call, which is the class of invention
            // that put `sort(by)` and `group_by(desc)` on this page. The
            // placeholder says whose the name is.
            if p.ty == crate::catalogue::SigTy::Aggregate {
                return format!("<name> = {}{repeat}", p.ty.page_name());
            }
            format!("{}{sep} {}{repeat}", p.name, p.ty.page_name())
        })
        .collect();
    format!("({}) -> {}", params.join(", "), sig.ret.page_name())
}

/// What a reader writes to call this row.
///
/// Derived from the head of the name, not chosen: a NAMESPACE row has only its
/// dotted spelling, because a namespace is not a value and there is nothing for
/// it to be on the right of. Anything else is reached through the value or
/// through the type — one row, two ways in — and the member is the half both
/// spellings share.
#[must_use]
pub fn call_spelling(name: &str) -> &str {
    match name.split_once('.') {
        Some((head, member)) if !is_namespace_head(head) => member,
        _ => name,
    }
}

/// Make `text` safe inside a Markdown table cell.
///
/// One escape, and it is load-bearing: GFM splits a row on unescaped `|`, and
/// the measured templates are regex alternations full of them. Unescaped,
/// `str.strip_html` would silently become a five-column row in a three-column
/// table.
///
/// # Panics
///
/// If `text` contains a backtick, which would close the code span it is printed
/// inside. No template does today; a template that did would need a different
/// fence and should say so rather than emit broken Markdown.
fn cell(text: &str) -> String {
    assert!(
        !text.contains('`'),
        "a catalogue row contains a backtick and cannot be printed in a code span: {text}"
    );
    text.replace('|', "\\|")
}

/// How a row compiles, in one cell.
fn lowering(l: &Lowering) -> String {
    match l {
        Lowering::Op(op) => format!("the `{op}` operator"),
        Lowering::Expr(template) => format!("`{}`", cell(template)),
    }
}

/// The header the partial carries. See `crate::catalogue::header` for the Rust
/// one; this is the MDX comment form, because MDX has no HTML comments.
fn header() -> String {
    "{/* @generated by `cargo xtask catalogue`. DO NOT EDIT.\n\
     \n\
     Every section below is one table and nothing else. The prose that explains\n\
     it — what a row does, why it takes what it takes, which four `math` rows go\n\
     inside `group_by` — is written by hand on `content/docs/book/stdlib.mdx`,\n\
     which pulls each section in with `<include>`.\n\
     \n\
     Source: `catalogue.bnf`, all of it. Change it and re-run the command. */}\n"
        .to_owned()
}

/// One `<section id="…">` wrapping one table.
///
/// The `id` is what `<include>…#id</include>` selects: `remarkInclude` looks for
/// a `<section>` element carrying it and replaces the include with that
/// section's children. Blank lines inside the element are required — without
/// them MDX reads the table as literal text rather than as Markdown.
fn section(out: &mut String, id: &str, header: &str, rows: &[[String; 3]]) {
    let _ = writeln!(out, "\n<section id=\"{id}\">\n");
    let _ = writeln!(out, "{header}");
    let _ = writeln!(out, "| --- | --- | --- |");
    for [a, b, c] in rows {
        let _ = writeln!(out, "| {a} | {b} | {c} |");
    }
    out.push_str("\n</section>\n");
}

/// The rows of one receiver's table, sorted by name.
fn signature_rows(entries: &[&FnRow]) -> Vec<[String; 3]> {
    entries
        .iter()
        .map(|e| {
            [
                format!("`{}`", call_spelling(&e.name)),
                format!("`{}`", cell(&signature(&e.sig))),
                lowering(&e.lowering),
            ]
        })
        .collect()
}

/// The `io` table's rows — from the `row` half of the file, in file order.
///
/// The `io` table prints what a row READS rather than its signature, because
/// that is what distinguishes one constructor from another: every one of them
/// takes a `uri` and gives back rows. The signature is checked, not shown.
fn io_rows(rows: &[Row]) -> Vec<[String; 3]> {
    rows.iter()
        .map(|row| {
            let extensions: Vec<String> = row
                .extensions
                .iter()
                .map(|e| format!("`.{}`", cell(e)))
                .collect();
            let reads = match (&row.reads, &row.decodes) {
                (Some(Reads::Native(f)), _) => format!("rows, through `{}`", cell(f)),
                (Some(Reads::Materialised), _) => {
                    "rows, materialised outside the reader".to_owned()
                }
                (None, Some(f)) => format!("types, through `{}`", cell(f)),
                (None, None) => unreachable!("catalogue::parse rejects a row with no capability"),
            };
            [
                format!("`io.{}`", cell(&row.name)),
                extensions.join(" "),
                reads,
            ]
        })
        .collect()
}

/// Every stdlib row, grouped by receiver head and sorted within each group.
///
/// Public so the guard can ask the same question the emitter answers, rather
/// than re-deriving the grouping and agreeing with itself.
#[must_use]
pub fn by_head(cat: &Catalogue) -> BTreeMap<&str, Vec<&FnRow>> {
    let mut grouped: BTreeMap<&str, Vec<&FnRow>> = BTreeMap::new();
    for row in &cat.fns {
        grouped.entry(head_of(&row.name)).or_default().push(row);
    }
    for group in grouped.values_mut() {
        group.sort_by(|a, b| a.name.cmp(&b.name));
    }
    grouped
}

/// The whole partial: one section per receiver, in alphabetical order.
///
/// The order between sections is not the order the page shows them in — the
/// page includes each by id and arranges them around its own prose. This one is
/// alphabetical so that the file is stable under any change to the page.
#[must_use]
pub fn emit_stdlib_reference(cat: &Catalogue) -> String {
    let grouped = by_head(cat);

    // `io` is a section of its own: its rows print extensions and readers, not
    // signatures, so it is not one of the grouped heads even though the four
    // data rows now carry a signature like any other.
    let mut ids: BTreeSet<&str> = grouped.keys().copied().collect();
    ids.insert("io");

    let mut out = header();
    for id in ids {
        if id == "io" {
            section(
                &mut out,
                "io",
                "| | extensions | what it reads |",
                &io_rows(&cat.rows),
            );
        } else {
            let entries = grouped
                .get(id)
                .expect("every id but `io` came from the grouping");
            section(
                &mut out,
                id,
                "| | signature | lowers to |",
                &signature_rows(entries),
            );
        }
    }
    out
}
