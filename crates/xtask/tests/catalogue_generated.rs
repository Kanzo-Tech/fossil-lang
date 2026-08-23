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
//! - **That the reference page's prose is true.** The tables below the fold are
//!   generated and therefore cannot be wrong; the sentences between them are
//!   written by hand and no test reads English. What the last three tests here
//!   buy is narrower and precise: the page cannot go back to writing a ROW by
//!   hand, and a row cannot quietly stop reaching it.

use xtask::catalogue::{self, Reads};
use xtask::reference;

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

// ── The reference page ─────────────────────────────────────────────────────
//
// `apps/docs/content/docs/book/stdlib.mdx` used to write the catalogue out by
// hand. The measurement that ended that is in `xtask::reference`'s module doc:
// 51 rows in the registry, 58 on the page, seven of the page's naming nothing
// the checker knows. These three tests are what stops it happening twice.

/// The page pulls in every section the partial emits, and writes none of them
/// itself.
///
/// Two halves, and the second is the one with teeth. A `<include>` that names a
/// section the partial does not have fails the docs build, so the first half is
/// belt and braces; a hand-written table row does NOT fail any build, which is
/// exactly how the 58 rows accumulated.
///
/// **What it cannot prove:** that the page includes a section in the right
/// place, or that the prose around it describes the rows below it. A section
/// pasted under the wrong heading passes here.
#[test]
fn the_reference_page_includes_the_partial_and_writes_no_row_itself() {
    let root = catalogue::repo_root();
    let page = std::fs::read_to_string(root.join("apps/docs/content/docs/book/stdlib.mdx"))
        .expect("the reference page is on disk");
    let partial = std::fs::read_to_string(root.join(reference::PARTIAL))
        .expect("the generated partial is on disk");

    let ids: Vec<&str> = partial
        .lines()
        .filter_map(|l| l.trim().strip_prefix("<section id=\""))
        .filter_map(|rest| rest.split('"').next())
        .collect();
    // Without this the two loops below would sweep a corpus of zero and report
    // it as a clean page — the vacuous pass this file's second test already
    // exists to prevent, one level up.
    assert!(
        ids.len() >= 8,
        "the partial declares {} section(s); the catalogue has at least eight receivers",
        ids.len()
    );

    for id in &ids {
        let include = format!("<include>../../generated/stdlib.mdx#{id}</include>");
        assert!(
            page.contains(&include),
            "the page never includes section `{id}`; write `{include}`"
        );
    }

    // A catalogue row is a table row carrying an arrow. The two example tables
    // under «How to read this page» carry call spellings and no arrow, which is
    // what keeps them out of this.
    for (n, line) in page.lines().enumerate() {
        assert!(
            !(line.starts_with('|') && line.contains("->")),
            "apps/docs/content/docs/book/stdlib.mdx:{} writes a catalogue row by hand:\n  {line}\n\
             rows come from `cargo xtask catalogue`; the page carries the prose",
            n + 1
        );
    }
}

/// Every row of the registry reaches the page.
///
/// The emitter groups by receiver head and prints one table per group, so a row
/// whose head nothing includes would vanish silently — the partial would simply
/// be one section shorter and would still match itself, which is precisely the
/// failure `every_row_lands_in_exactly_one_generated_file` guards for the Rust
/// halves.
///
/// **What it cannot prove:** that the row's SIGNATURE is right. That
/// `str.slice` should take an `end` is a decision, and `stdlib.rs` is where it
/// is argued; this proves the page says what the registry says.
#[test]
fn every_registry_row_reaches_the_reference_page() {
    let partial = std::fs::read_to_string(catalogue::repo_root().join(reference::PARTIAL))
        .expect("the generated partial is on disk");

    let grouped = reference::by_head();
    assert!(
        grouped.values().map(Vec::len).sum::<usize>() >= 40,
        "the registry yielded almost nothing; a guard over an empty catalogue passes vacuously"
    );

    for entries in grouped.values() {
        for entry in entries {
            let cell = format!("| `{}` |", reference::call_spelling(entry));
            assert!(
                partial.contains(&cell),
                "`{}` is in the registry and not on the page",
                entry.name
            );
        }
    }
}

/// The `io` section comes from `catalogue.bnf` and not from the registry, and
/// this is the claim that makes choosing safe rather than merely deliberate:
/// every `io.` row the registry carries is a `catalogue.bnf` row too.
///
/// The registry has three (`csv`, `json`, `parquet`) against the file's six —
/// they are stubs saying «this name is a source», and the file is the datum.
/// Printing the registry's three would have deleted `io.rdf`, `io.shex` and
/// `io.shacl` from the page. If the subset ever stops holding, a name exists
/// that this page cannot show, and the choice of source has to be reopened.
///
/// **What it cannot prove:** the converse. `io.rdf` has no registry row and
/// binds perfectly well, so a file row without a registry row is normal.
#[test]
fn every_registry_io_row_is_a_catalogue_row() {
    let names: Vec<String> = rows().iter().map(|r| r.name.clone()).collect();
    let io = reference::by_head();
    let io = io.get("io").expect("the registry carries `io.` rows");
    assert!(!io.is_empty(), "an empty `io` group would pass vacuously");

    for entry in io {
        let member = entry.name.split_once('.').expect("a dotted name").1;
        assert!(
            names.iter().any(|n| n == member),
            "the registry carries `{}` and `catalogue.bnf` has no `{member}` row, \
             so the page's `io` table cannot show it",
            entry.name
        );
    }
}
