//! **What a source header is told about the reader option it wrote.**
//!
//! `io.csv("u.csv", delimiter = "|")` is the first argument of a source binding
//! that the READER has to honour, and the position it fills is declared in
//! `catalogue.bnf` — `delimiter = String?`, named because the call site writes
//! the name and optional because leaving it off is a legal program.
//!
//! A source binding's arguments are read by a token scan and not by the
//! signature binder (`crate::def_map::parse_reader_option`), which means the
//! default answer to a badly written one is SILENCE: the scan finds nothing and
//! the reader gets nothing. Silence is the wrong answer for all three shapes
//! below, and this file is what holds each of them to a sentence.
//!
//! # What it does NOT prove
//!
//! - **That the option reaches either engine.** `fossil-mir` carries it into
//!   `SourceFormat::Csv` and `fossil-df` turns it into a `CsvReadOptions`
//!   delimiter; both have their own tests, and
//!   `apps/docs/programs/pipe-delimited` is the one artefact that proves the
//!   whole thread at once.
//! - **That the WORD is `delimiter`.** It is the catalogue's, read back by
//!   `reader_option_of`; `crates/fossil-hir/src/def_map.rs`'s own
//!   `the_option_the_scanner_reads_is_the_one_the_catalogue_declares` is what
//!   holds the derivation to the file. This file writes the word because a
//!   program written by an author writes it.

use fossil_base::test_support::{db_with_document, register_inferred};
use fossil_base::{Diagnostic, FossilDb, SourceFile};
use fossil_graph_schema::Primitive;
use fossil_hir::lower::lower_to_hir;

const DOCUMENT: &str = "\
shape http://example.org/Persona
prop http://example.org/name - 1 1
";

const COLUMNS: &[(&str, Primitive)] = &[("id", Primitive::Integer), ("name", Primitive::String)];

/// A program whose only interesting line is the binding under test.
fn program(binding: &str) -> String {
    format!(
        "type {{ Persona }} := io.shex(\"v.shex\")\n\
         {binding}\n\
         Venta : Persona from rows\n    \
         @subject = \"https://example.org/v/{{rows.id}}\"\n    \
         name = rows.name\n"
    )
}

fn diagnostics(binding: &str) -> Vec<String> {
    let (db, file): (FossilDb, SourceFile) =
        db_with_document(&program(binding), "v.shex", DOCUMENT);
    register_inferred(&db, "u.csv", COLUMNS);
    register_inferred(&db, "u.json", COLUMNS);
    let _ = lower_to_hir(&db, file);
    lower_to_hir::accumulated::<Diagnostic>(&db, file)
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// Exactly one message containing `needle`, so a test cannot pass by finding it
/// inside a list of twenty.
fn one_about(binding: &str, needle: &str) -> String {
    let all = diagnostics(binding);
    let hits: Vec<&String> = all.iter().filter(|m| m.contains(needle)).collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one diagnostic containing {needle:?}, got {}:\n{all:#?}",
        hits.len()
    );
    hits[0].clone()
}

/// **The delimiter a program writes correctly is not a diagnostic.**
///
/// First, because it is the ordinary case; and second because every assertion
/// below is about a message, and a file where the healthy program also produced
/// one would be measuring something other than what it claims.
#[test]
fn a_well_written_delimiter_is_silent() {
    assert_eq!(
        diagnostics("rows := io.csv(\"u.csv\", delimiter = \"|\")"),
        Vec::<String>::new()
    );
}

/// **A binding that names no delimiter is silent too.** Absent is a legal
/// program — that is what `?` says in `catalogue.bnf` — and it is what every
/// program written before the position existed says.
#[test]
fn no_delimiter_at_all_is_still_a_program() {
    assert_eq!(
        diagnostics("rows := io.csv(\"u.csv\")"),
        Vec::<String>::new()
    );
}

/// **`io.json` has no delimiter, and saying so is the point.**
///
/// Only `io.csv`'s catalogue row declares the position, so on any other row the
/// argument is read by nothing and changes nothing. Leaving it unreported is
/// precisely the defect the delimiter exists to end, one row over: an option
/// the author believes is in force and no reader ever sees.
#[test]
fn a_delimiter_on_a_row_that_has_none_is_reported_and_names_the_row_that_does() {
    let m = one_about(
        "rows := io.json(\"u.json\", delimiter = \"|\")",
        "delimiter",
    );
    assert!(
        m.contains("`io.json` has no `delimiter`"),
        "the row that has no such position must be named: {m}"
    );
    assert!(
        m.contains("`io.csv` does"),
        "and so must the one that does, or the author has nowhere to go: {m}"
    );
}

/// **A delimiter has to be one ASCII character**, because both readers a corpus
/// is written through take one byte: `DuckDB`'s `delim=` and `DataFusion`'s
/// `CsvReadOptions::delimiter`. A wider one would mean something different to
/// each, which is the disagreement this whole parameter exists to remove.
#[test]
fn a_delimiter_wider_than_one_character_is_refused() {
    let m = one_about("rows := io.csv(\"u.csv\", delimiter = \"||\")", "delimiter");
    assert!(
        m.contains("one ASCII character"),
        "the rule has to be stated, not just the refusal: {m}"
    );
    assert!(m.contains("\"||\""), "and the value quoted back: {m}");
}

/// **One `char` is not one byte, and the rule is the byte.**
///
/// `§` is a single character and two bytes, so a `chars().count() == 1` check
/// would pass it here and `fossil-df` would then have nothing to give
/// `CsvReadOptions::delimiter`, which takes a `u8` — an option accepted by the
/// checker and dropped by the reader, which is the exact defect the whole
/// thread is about. The rule is stated in bytes on both sides.
#[test]
fn a_one_char_two_byte_delimiter_is_refused_because_the_reader_takes_a_byte() {
    let m = one_about("rows := io.csv(\"u.csv\", delimiter = \"§\")", "delimiter");
    assert!(
        m.contains("2 bytes"),
        "the width has to be named in the unit the rule is in: {m}"
    );
}

#[test]
fn an_empty_delimiter_is_refused_as_the_same_rule() {
    let m = one_about("rows := io.csv(\"u.csv\", delimiter = \"\")", "delimiter");
    assert!(m.contains("empty"), "an empty value says so by name: {m}");
}

/// A value that is not a string literal never reaches the scan's `STRING` arm,
/// so without this it would be an option the author wrote and nobody read.
#[test]
fn a_delimiter_that_is_not_a_string_is_refused() {
    let m = one_about("rows := io.csv(\"u.csv\", delimiter = 7)", "delimiter");
    assert!(
        m.contains("is not a string"),
        "the shape the position takes has to be named: {m}"
    );
}
