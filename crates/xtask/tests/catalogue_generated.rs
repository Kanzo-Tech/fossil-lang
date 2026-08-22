//! **The checked-in generated files are what `catalogue.bnf` says.**
//!
//! This replaces `fossil-descriptors-output/tests/catalogue_parity.rs`, which
//! parsed the same rows and compared them to hand-written statics. Under
//! generation every one of its four assertions is an identity — the extensions
//! in the static ARE the extensions in the file, because one was printed from
//! the other — so what is left to check is the thing that can still be wrong:
//! that somebody edited `catalogue.bnf` and did not re-run the generator, or
//! edited a generated file by hand.
//!
//! It lives here rather than beside a provider crate because the reason the old
//! one had to sit in `fossil-descriptors-output` has evaporated. That test
//! needed a crate that could SEE all six rows as Rust items; a generator reads
//! the file, so it sees every row from anywhere.
//!
//! # What this cannot prove
//!
//! - **That the rows are right.** That `io.csv` should accept `.csv` and not
//!   `.tsv` is a decision, and `catalogue.bnf`'s commentary is where it is
//!   argued. This proves the Rust says what the file says.
//! - **That a `decodes` function does what its row claims.** The compiler now
//!   proves the NAME resolves — which the parity test explicitly could not —
//!   but `decode_shex` returning nonsense is a `fossil-shex` test's problem.
//! - **That every generated file is listed.** `catalogue::generated()` is the
//!   list; a target added to the generator and forgotten there is invisible to
//!   this, exactly as it is to `--check`.

use xtask::catalogue::{self, Reads};

/// The generator's own view of the file, parsed once.
fn rows() -> Vec<catalogue::Row> {
    let text = std::fs::read_to_string(catalogue::repo_root().join("catalogue.bnf"))
        .expect("read catalogue.bnf");
    catalogue::parse(&text)
}

/// The check `cargo xtask catalogue --check` runs, as a test — so a stale file
/// fails in `cargo test` and not only in whatever CI step remembers to invoke
/// the binary.
#[test]
fn the_checked_in_files_match_the_catalogue() {
    let root = catalogue::repo_root();
    for (path, want) in catalogue::generated() {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let have = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{shown} is generated and must exist: {e}"));
        assert_eq!(
            have, want,
            "{shown} is stale — run `cargo xtask catalogue` and commit the result"
        );
    }
}

/// The guard has to be able to fail. Carried over from the parity test, and for
/// the reason it gave: if the parse silently found nothing, every assertion
/// here would compare two empty things and pass — which is how `alpha-steps.test.ts`
/// in the sibling repository spent weeks matching a corpus of zero.
#[test]
fn the_parse_actually_read_the_file() {
    let rows = rows();
    assert!(
        rows.len() >= 6,
        "catalogue.bnf parsed to {} rows; the file declares at least six",
        rows.len()
    );
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"csv") && names.contains(&"shex"),
        "the parse missed rows it must see: {names:?}"
    );
}

/// Every row reaches exactly one of the two generated files, and the split is
/// the one `catalogue.bnf` argues for: a row that `decodes` needs a crate that
/// links a shape-language parser.
///
/// Without this, a row whose clause the parser did not understand would be
/// dropped from both files and nothing above would notice — the emitters would
/// simply print one row fewer, and their output would still match itself.
#[test]
fn every_row_lands_in_exactly_one_generated_file() {
    let rows = rows();
    let base = catalogue::emit_base(&rows);
    let descriptors = catalogue::emit_descriptors(&rows);

    for row in &rows {
        let ident = row.name.to_ascii_uppercase();
        let in_base = base.contains(&format!("pub static {ident}:"));
        let in_descriptors = descriptors.contains(&format!("pub static {ident}:"));
        assert!(
            in_base ^ in_descriptors,
            "row `{}` is in {} generated file(s), expected exactly one",
            row.name,
            u8::from(in_base) + u8::from(in_descriptors)
        );
        assert_eq!(
            in_descriptors,
            row.decodes.is_some(),
            "row `{}`: a row that decodes belongs in fossil-descriptors-output, \
             and only such a row does",
            row.name
        );
    }
}

/// A `native <fn>` token is the only place the reader is named: the variant is
/// derived from it and `table_function` gives it back.
///
/// Proven against a row the file does not contain, so this measures the
/// derivation rather than agreeing with whatever the four real rows happen to
/// spell.
#[test]
fn a_new_native_reader_is_a_row_and_nothing_else() {
    let invented =
        catalogue::parse("row avro = extensions \"avro\" ; reads native read_avro_scan .\n");
    assert_eq!(invented.len(), 1);
    assert_eq!(
        invented[0].reads,
        Some(Reads::Native("read_avro_scan".into()))
    );

    let emitted = catalogue::emit_base(&invented);
    assert!(
        emitted.contains("AvroScan,"),
        "the variant name is derived from the token: {emitted}"
    );
    assert!(
        emitted.contains("Self::AvroScan => \"read_avro_scan\""),
        "and `table_function` gives the token back: {emitted}"
    );
    assert!(
        emitted.contains("reads_rows: Some(RowReader::Native(NativeReader::AvroScan))"),
        "and the row points at it: {emitted}"
    );
}
