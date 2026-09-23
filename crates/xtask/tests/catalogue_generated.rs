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
    catalogue::read().rows
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
        catalogue::parse("row avro = extensions \"avro\" ; reads native read_avro_scan .\n").rows;
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
    // Six: `core`, `io`, `math`, `parse`, `seq`, `str`. It was eight, and the
    // two that left are `validate` and `anon` — whole namespaces, not rows.
    assert!(
        ids.len() >= 6,
        "the partial declares {} section(s); the catalogue has at least six receivers",
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

/// Every stdlib row reaches the page.
///
/// The emitter groups by receiver head and prints one table per group, so a row
/// whose head nothing includes would vanish silently — the partial would simply
/// be one section shorter and would still match itself, which is precisely the
/// failure `every_row_lands_in_exactly_one_generated_file` guards for the Rust
/// halves.
///
/// **What it cannot prove:** that the row's SIGNATURE is right. That
/// `str.slice` should take an `end` is a decision, and `catalogue.bnf` is where
/// it is argued; this proves the page says what the file says.
#[test]
fn every_stdlib_row_reaches_the_reference_page() {
    let partial = std::fs::read_to_string(catalogue::repo_root().join(reference::PARTIAL))
        .expect("the generated partial is on disk");

    let cat = catalogue::read();
    let grouped = reference::by_head(&cat);
    assert!(
        grouped.values().map(Vec::len).sum::<usize>() >= 30,
        "the catalogue yielded almost nothing; a guard over an empty one passes vacuously"
    );

    for entries in grouped.values() {
        for entry in entries {
            let cell = format!("| `{}` |", reference::call_spelling(&entry.name));
            assert!(
                partial.contains(&cell),
                "`{}` is in the catalogue and not on the page",
                entry.name
            );
        }
    }
}

// ── The round trip ─────────────────────────────────────────────────────────
//
// These are what make `fossil-hir` a DEV-dependency rather than a deleted one.
// `cargo xtask catalogue` generates that crate's stdlib table, so the binary
// must not link it — a bad emit would stop the tool that fixes it from
// building. But the emit still has to be PROVEN faithful, and the only way to
// prove it is to hold the generated table against the registry it becomes. A
// dev-dependency buys both: the tests link fossil-hir, the binary does not.

/// **Every row in `catalogue.bnf` is in the registry with the signature and the
/// lowering the file gives it — and the registry carries nothing else.**
///
/// The one guard that would catch the emitter dropping a row, reordering a
/// parameter, losing an arity or a type, or mangling a template.
/// `str.strip_html` is what makes the last one concrete: its template carries
/// double quotes, and the line scanner this replaces would have truncated it
/// silently rather than failing.
///
/// The comparisons are derived rather than restated. An `Arity` and a `SigTy`
/// are two enums with the same variant names on either side of the generator,
/// so `Debug` is the shared spelling and neither side needs a translation table
/// that could itself drift.
#[test]
fn the_generated_table_is_the_file() {
    use fossil_hir::stdlib::{LoweringKind, stdlib};

    let cat = catalogue::read();
    let reg = stdlib();
    let declared = cat.registry_rows();
    assert!(
        declared.len() >= 30,
        "the catalogue parsed to {} row(s); a comparison over nothing passes vacuously",
        declared.len()
    );

    for (name, sig, lowering) in &declared {
        let entry = reg
            .lookup(name)
            .unwrap_or_else(|| panic!("`{name}` is in catalogue.bnf and not in the registry"));

        assert_eq!(
            entry.sig.params.len(),
            sig.params.len(),
            "`{name}`: {} parameter(s) in the registry, {} in the file",
            entry.sig.params.len(),
            sig.params.len()
        );
        for (got, want) in entry.sig.params.iter().zip(&sig.params) {
            assert_eq!(got.name.as_str(), want.name, "`{name}`: parameter name");
            assert_eq!(
                got.named, want.named,
                "`{name}` `{}`: named-ness",
                want.name
            );
            assert_eq!(
                format!("{:?}", got.arity),
                format!("{:?}", want.arity),
                "`{name}` `{}`: arity",
                want.name
            );
            assert_eq!(
                format!("{:?}", got.ty),
                debug_of(want.ty),
                "`{name}` `{}`: type",
                want.name
            );
        }
        assert_eq!(
            format!("{:?}", entry.sig.ret),
            debug_of(sig.ret),
            "`{name}`: return type"
        );

        match (&entry.lowering, lowering) {
            (LoweringKind::Expr(got), catalogue::Lowering::Expr(want)) => assert_eq!(
                got.as_str(),
                want.as_str(),
                "`{name}`: the template did not round-trip"
            ),
            (LoweringKind::Op(got), catalogue::Lowering::Op(want)) => assert_eq!(
                format!("{got:?}"),
                *want,
                "`{name}`: the operator did not round-trip"
            ),
            (got, want) => {
                panic!("`{name}`: lowering kind differs — registry {got:?}, file {want:?}")
            }
        }
    }

    // And nothing the file does not declare. The table is generated, so a row
    // here that no row declares could only have arrived by somebody editing a
    // file whose header says DO NOT EDIT.
    let names: Vec<&str> = declared.iter().map(|(n, _, _)| n.as_str()).collect();
    for entry in reg.iter() {
        assert!(
            names.contains(&entry.name.as_str()),
            "`{}` is in the registry and in no `catalogue.bnf` row",
            entry.name
        );
    }
}

/// `fossil_hir::stdlib::SigTy`'s `Debug` spelling of one of the file's types.
///
/// `rust_path` is what the emitter writes; dropping the two type qualifiers off
/// it is exactly `Debug`'s output for the same value. Derived from the emitter's
/// own answer so that a new type spelling cannot be added here and forgotten
/// there.
fn debug_of(ty: catalogue::SigTy) -> String {
    ty.rust_path()
        .replace("SigTy::", "")
        .replace("ScalarTy::", "")
}

/// `is_namespace_head` agrees with `fossil_hir::stdlib::receiver_of`.
///
/// `xtask` states the receiver rule a second time — it needs one bit of it to
/// decide whether the page prints a row's dotted name or its member, and it may
/// not link `fossil-hir` to ask. A second statement of a rule is a second thing
/// to keep in step, so this derives the comparison from the original rather than
/// asserting the answer.
#[test]
fn the_receiver_rule_agrees_with_fossil_hir() {
    use fossil_hir::stdlib::{Receiver, receiver_of};

    let cat = catalogue::read();
    let mut heads: Vec<&str> = cat
        .fns
        .iter()
        .filter_map(|f| f.name.split_once('.').map(|(h, _)| h))
        .collect();
    heads.sort_unstable();
    heads.dedup();
    assert!(heads.len() >= 4, "only {} head(s) found", heads.len());

    for head in heads {
        assert_eq!(
            catalogue::is_namespace_head(head),
            receiver_of(head) == Receiver::Namespace,
            "`{head}`: xtask and fossil-hir disagree about whether it is a namespace"
        );
    }
    // A head neither side has seen must still agree, or the two rules only
    // happen to match on the corpus that exists.
    assert!(catalogue::is_namespace_head("invented"));
    assert_eq!(receiver_of("invented"), Receiver::Namespace);
}

/// `io.shex` and `io.shacl` carry NO signature, and that is a decision rather
/// than an oversight — so it is pinned.
///
/// They give back a shape document, which `SigTy` has no spelling for. Writing
/// `-> Rows` to fill the column would repeat the defect the `add`/`add_verb`
/// split once had, where three constructors of relations declared `String`.
/// Every row that reads DATA does carry one, `io.rdf` included — which is what
/// the fold fixed: it had none, so writing it in a value position went
/// undiagnosed.
#[test]
fn exactly_the_data_rows_carry_a_signature() {
    let rows = rows();
    assert!(rows.len() >= 6, "an empty row set would pass vacuously");
    for row in rows {
        assert_eq!(
            row.call.is_some(),
            row.reads.is_some(),
            "`io.{}`: a row that reads DATA needs a signature, and a row that \
             decodes a shape language cannot have one",
            row.name
        );
    }
}
