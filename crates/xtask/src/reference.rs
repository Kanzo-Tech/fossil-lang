//! The catalogue, projected onto the reference page — `apps/docs/content/generated/stdlib.mdx`.
//!
//! # Why this exists
//!
//! The catalogue is DATA and a compiler reads it. `crate::catalogue` already
//! does that for the `io.` half —
//! `catalogue.bnf` in, four projections out. The stdlib half never became a
//! file, but it never needed to in order to be data: `fossil_hir::stdlib`'s
//! `FunctionRegistry` is a table of `RegistryEntry { name, recv, member, sig,
//! lowering }`, five fields and every one of them a value. What it lacked was a
//! consumer other than the checker.
//!
//! `apps/docs/content/docs/book/stdlib.mdx` was that table written a second
//! time, by hand, in Markdown — and the second copy had drifted, which is what
//! a second copy does. Measured on 2026-08-23, before this file existed:
//!
//! - 51 rows in the registry, **every one of them on the page**;
//! - 58 distinct rows on the page, so **seven named nothing the checker knows**;
//! - of those seven, `io.rdf`, `io.shex` and `io.shacl` are real language
//!   surface that lives in the OTHER half (`catalogue.bnf`, resolved through
//!   `fossil_base::providers`, never through the registry); `seq.filter` was
//!   renamed to `seq.where`; `str.normalize_unicode` was deleted from the
//!   language outright; and `io.sql` and `io.http` occur nowhere in the
//!   repository at all — the page is their only existence.
//!
//! The seven are the argument for generating this. A reader could not have told
//! them apart from the 51, because the page rendered all 58 identically.
//!
//! **Twelve of the 51 that did exist described a call the checker rejects** —
//! wrong arity, or a named argument naming no parameter of the row: `dir` on
//! `core.lang`, `digits` on `math.round`, `end` on `str.slice`, `delimiter` and
//! `header` on `io.csv`, `schema` on `parse.json` (the row says `path`),
//! `delimiter` on `parse.csv_row` (`separator`, and the field index was
//! missing), `from`/`to` on `str.replace` (`needle`/`replacement`), `sep` on
//! `str.split` (`separator`), `by`/`key`/`desc` on `seq.distinct`,
//! `seq.group_by` and `seq.sort`, which have no such parameters, and
//! `str.concat` written variadic where it takes two. That is why `signature`
//! below prints every parameter WITH ITS NAME: the other 39 rows differed only
//! in notation, and a page that cannot be told apart from one that is wrong is
//! not a reference.
//!
//! # What this does NOT generate, deliberately
//!
//! The ARGUMENT — the same line `crate::catalogue` draws, for the same reason.
//! `book/stdlib.mdx` keeps every sentence: what `slug` does to a string, which
//! four `math` rows go inside `aggregate`, why `io`'s constructors take nothing
//! on the left. A generator that tried to carry prose would make the registry a
//! documentation format with a type checker attached to it.
//!
//! What crosses the line is a NAME, a SIGNATURE, an EXTENSION and a LOWERING.
//! Each is a field of a row; none of them is a sentence.
//!
//! # Two sources, because `io.` is two halves
//!
//! Every section but `io` comes from the registry. `io` comes from
//! `catalogue.bnf`, and that is not an inconsistency — it is where the datum
//! is. The registry carries three `io.` rows (`csv`, `json`, `parquet`) with a
//! stub signature `(uri: String) -> String`, because what a `HirExpr::Call`
//! needs from them is «this name is a source, not a scalar expression».
//! **Which** `io.` names exist, what each accepts and what each reads is
//! `catalogue.bnf`'s answer and always has been — `io.rdf` has no registry row
//! and binds perfectly well. Printing the registry's three would have deleted
//! half the readers from the page.
//!
//! `crates/xtask/tests/catalogue_generated.rs` holds the subset that makes the
//! split safe rather than merely stated: every registry `io.` row must be a
//! `catalogue.bnf` row too.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use fossil_hir::stdlib::{Arity, LoweringKind, Receiver, RegistryEntry, ScalarTy, SigTy};

use crate::catalogue::{Reads, Row};

/// Where the emitted partial goes, repo-relative.
///
/// Outside `content/docs/`, which is the whole of why it is here and not beside
/// the page: `defineDocs({ dir: "content/docs" })` routes every `.mdx` under
/// that directory, so a partial living there would be a page of the site.
/// `remarkInclude` resolves `<include>` against the INCLUDING file's directory,
/// so `book/stdlib.mdx` reaches it as `../../generated/stdlib.mdx`.
pub const PARTIAL: &str = "apps/docs/content/generated/stdlib.mdx";

/// The receiver head of a dotted catalogue name — `str` of `str.trim`.
///
/// The same split `fossil_hir::stdlib::split_receiver` makes, kept to its left
/// half: this groups rows into tables, it does not classify receivers.
fn head_of(name: &str) -> &str {
    name.split_once('.').map_or(name, |(head, _)| head)
}

/// How a signature position is spelled on the page.
///
/// `Rows` and `Predicate` are not scalars and have no `ScalarTy` to fall back
/// on — both depend on the receiver, which is exactly why `SigTy` has them as
/// variants. They are printed under the names the type page uses.
const fn ty_name(t: SigTy) -> &'static str {
    match t {
        SigTy::Scalar(ScalarTy::String) => "String",
        SigTy::Scalar(ScalarTy::Integer) => "Integer",
        SigTy::Scalar(ScalarTy::Float) => "Float",
        SigTy::Scalar(ScalarTy::Bool) => "Bool",
        SigTy::Scalar(ScalarTy::Date) => "Date",
        SigTy::Scalar(ScalarTy::DateTime) => "DateTime",
        SigTy::Scalar(ScalarTy::SeqString) => "Seq<String>",
        SigTy::Rows => "Relation",
        SigTy::Predicate => "Predicate",
        SigTy::Column => "Column",
        SigTy::Binding => "Binding",
    }
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
/// unprintable while `Arity` and `ParamSpec::named` did not exist, and their
/// absence is what let this page write `sort(by)`, `distinct(key)` and
/// `group_by(desc)` — three parameters no row had. It now prints
/// `sort(rows: Relation, by: Column…)`, which is the row.
fn signature(entry: &RegistryEntry) -> String {
    let params: Vec<String> = entry
        .sig
        .params
        .iter()
        .map(|p| {
            // `on = Predicate`, not `on: Predicate`. The separator is the one
            // the call site writes, so a reader copying the signature copies a
            // program that parses.
            let sep = if p.named { " =" } else { ":" };
            let repeat = match p.arity {
                Arity::One => "",
                Arity::OneOrMore => "…",
                Arity::Optional => "?",
            };
            format!("{}{sep} {}{repeat}", p.name, ty_name(p.ty))
        })
        .collect();
    format!("({}) -> {}", params.join(", "), ty_name(entry.sig.ret))
}

/// What a reader writes to call this row.
///
/// Derived from the receiver, not chosen: a [`Receiver::Namespace`] row has
/// only its dotted spelling, because a namespace is not a value and there is
/// nothing for it to be on the right of. Anything else is reached through the
/// value or through the type — one row, two ways in — and the member is the
/// half both spellings share.
#[must_use]
pub fn call_spelling(entry: &RegistryEntry) -> &str {
    if entry.recv == Receiver::Namespace {
        &entry.name
    } else {
        &entry.member
    }
}

/// Make `text` safe inside a Markdown table cell.
///
/// One escape, and it is load-bearing: GFM splits a row on unescaped `|`, and
/// four of the six measured templates are regex alternations full of them
/// (`'^…$|^[0-9a-fA-F]{32}$|…'`). Unescaped, `validate.uuid` would silently
/// become a five-column row in a three-column table.
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
///
/// Both variants of `LoweringKind`, printed as what they are. The operator name
/// comes off `Debug`, which is the variant's own spelling and the only name a
/// `PlanOp` has — there is no second table mapping variants to strings, and the
/// reason there is none is that `crate::catalogue`'s predecessor had exactly
/// that and it had already drifted.
fn lowering(entry: &RegistryEntry) -> String {
    match &entry.lowering {
        LoweringKind::Op(op) => format!("the `{op:?}` operator"),
        LoweringKind::Expr(template) => format!("`{}`", cell(template)),
    }
}

/// The header the partial carries. See `crate::catalogue::header` for the Rust
/// one; this is the MDX comment form, because MDX has no HTML comments.
fn header() -> String {
    "{/* @generated by `cargo xtask catalogue`. DO NOT EDIT.\n\
     \n\
     Every section below is one table and nothing else. The prose that explains\n\
     it — what a row does, why it takes what it takes, which four `math` rows go\n\
     inside `aggregate` — is written by hand on `content/docs/book/stdlib.mdx`,\n\
     which pulls each section in with `<include>`.\n\
     \n\
     Sources: `fossil_hir::stdlib`'s FunctionRegistry for every section but `io`,\n\
     and `catalogue.bnf` for `io`. Change either and re-run the command. */}\n"
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
///
/// Alphabetical because there is no other order to be faithful to:
/// `FunctionRegistry` stores its rows in a `HashMap`, so the sequence the
/// catalogue is WRITTEN in does not survive into the value this reads. An
/// emitter that picked its own grouping would be inventing one.
fn signature_rows(entries: &[&RegistryEntry]) -> Vec<[String; 3]> {
    entries
        .iter()
        .map(|e| {
            [
                format!("`{}`", call_spelling(e)),
                format!("`{}`", cell(&signature(e))),
                lowering(e),
            ]
        })
        .collect()
}

/// The `io` table's rows — from `catalogue.bnf`, in file order. See the module
/// doc for why this half comes from the other file.
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

/// Every registry row, grouped by receiver head and sorted within each group.
///
/// Public so the guard can ask the same question the emitter answers, rather
/// than re-deriving the grouping and agreeing with itself.
#[must_use]
pub fn by_head() -> BTreeMap<&'static str, Vec<&'static RegistryEntry>> {
    let mut grouped: BTreeMap<&'static str, Vec<&'static RegistryEntry>> = BTreeMap::new();
    for entry in fossil_hir::stdlib::stdlib().iter() {
        grouped.entry(head_of(&entry.name)).or_default().push(entry);
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
pub fn emit_stdlib_reference(catalogue_rows: &[Row]) -> String {
    let grouped = by_head();

    // `io` is a section whether or not the registry carries an `io.` row: its
    // rows come from `catalogue.bnf`, and the day the registry's three stubs go
    // the page must not lose the readers with them.
    let mut ids: BTreeSet<&str> = grouped.keys().copied().collect();
    ids.insert("io");

    let mut out = header();
    for id in ids {
        if id == "io" {
            section(
                &mut out,
                "io",
                "| | extensions | what it reads |",
                &io_rows(catalogue_rows),
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
