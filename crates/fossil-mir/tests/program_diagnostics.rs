//! What the one drain answers that a per-mapping loop cannot.
//!
//! [`fossil_mir::program_diagnostics`] exists because the question «what is
//! wrong with this program» was answered three ways, and two of the three — the
//! LSP's and the browser's — were a per-mapping loop and nothing else. Each
//! test below is one class of diagnostic that loop could not reach, phrased as
//! the user-visible consequence rather than as the query that produces it.
//!
//! **What this file cannot prove:** that any diagnostic here is the RIGHT
//! diagnostic. It asserts reachability — that the drain gets to a message at
//! all — because unreachability is the failure it exists against, and every
//! message's own wording is fixed where it is emitted (`fossil-syntax`'s
//! recovery corpus, `fossil-hir`'s diagnostic corpus). It also does not prove
//! the three hosts stay collapsed: nothing stops someone writing a fourth loop.
//! What makes that visible is that there is one `pub fn` and three call sites,
//! not a test.

#![cfg(not(target_arch = "wasm32"))]
// `{Row.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

use fossil_base::test_support;
use fossil_mir::program_diagnostics;

/// One un-narrowed `name` predicate — enough for a mapping to type-check.
const SHAPE: &str = "\
shape https://example.org/Person
prop https://example.org/name - 1 1
";

/// Two mappings producing ONE type from two `@subject` templates that disagree.
/// Nothing per-mapping can see this: `check_identity` is keyed by a mapping and
/// by construction holds one at a time.
const TWO_IDENTITIES: &str = "\
type { Person } := io.shex(\"p.shex\")

A := io.csv(\"a.csv\")
B := io.csv(\"b.csv\")

First : Person from A
    @subject = \"https://example.org/person/{A.id}\"
    name = A.name

Second : Person from B
    @subject = \"https://example.org/other/{B.id}\"
    name = B.name
";

/// A top-level binding naming a constructor no host installs, in a file with a
/// perfectly good mapping beside it. `lower_to_hir` raises this, and
/// `lower_to_hir` is not in `def_map`'s dependency subtree.
const BAD_PROVIDER: &str = "\
type { Person } := io.shex(\"p.shex\")

Rows := io.nonesuch(\"rows.xyz\")
Users := io.csv(\"users.csv\")

People : Person from Users
    @subject = \"https://example.org/person/{Users.id}\"
    name = Users.name
";

/// Garbage. The parser recovers no mapping at all, so a per-mapping loop runs
/// zero times and reports a clean file.
const NO_MAPPING: &str = "@@@ ??? := := :=\n";

fn diagnose(program: &str) -> Vec<String> {
    let (db, file) = test_support::db_with_document_at("t.fossil", program, "p.shex", SHAPE);
    program_diagnostics(&db, file)
        .into_iter()
        .map(|d| d.message)
        .collect()
}

/// **The parse errors were present and unreachable.** Salsa accumulates over a
/// query's dependency subtree and `parse` is only in the subtree of a mapping,
/// so a file with no mapping had them collected and never drained. An editor
/// showed a clean document for a file that does not parse.
#[test]
fn a_file_the_parser_recovered_no_mapping_from_still_reports() {
    let (db, file) = test_support::db_with_document_at("t.fossil", NO_MAPPING, "p.shex", SHAPE);
    assert!(
        fossil_hir::def_map::def_map(&db, file)
            .mappings(&db)
            .is_empty(),
        "the fixture must actually yield no mapping, or it proves nothing"
    );
    assert!(
        !program_diagnostics(&db, file).is_empty(),
        "a file that does not parse must not report clean"
    );
}

/// **A top-level binding's own diagnostics are not a mapping's.** They come out
/// of `lower_to_hir`, which a `def_map`-keyed drain never reaches and a
/// mapping-keyed one reaches only by accident.
#[test]
fn a_binding_that_names_no_provider_is_reported_beside_a_healthy_mapping() {
    let messages = diagnose(BAD_PROVIDER);
    assert!(
        messages.iter().any(|m| m.contains("nonesuch")),
        "the binding names a constructor no host installs and nothing said so; got {messages:?}"
    );
}

/// **Identity is a fact about the FILE.** Two mappings minting two `@subject`
/// templates for one type is two entities where there was one, and it is an
/// error — but only a file-keyed query can see both templates at once.
#[test]
fn two_mappings_minting_two_identities_for_one_type_are_reported() {
    let messages = diagnose(TWO_IDENTITIES);
    assert!(
        messages
            .iter()
            .any(|m| m.to_lowercase().contains("identit") || m.contains("@subject")),
        "one type with two identity templates must be reported; got {messages:?}"
    );
}

/// **One mistake, printed once.** The file-keyed queries sit in every mapping's
/// dependency subtree, so before the dedup a program with N mappings reported
/// each top-level mistake N times. `BAD_PROVIDER` has one bad binding and one
/// mapping; this asserts the general rule at the smallest size that can fail,
/// by counting occurrences of the one message rather than the length of the
/// list.
#[test]
fn a_file_level_mistake_is_reported_once_not_once_per_mapping() {
    let messages = diagnose(BAD_PROVIDER);
    let about_binding = messages.iter().filter(|m| m.contains("nonesuch")).count();
    assert_eq!(
        about_binding, 1,
        "one mistake about one binding, reported once; got {messages:?}"
    );
}

/// And the converse the dedup must not break: two DIFFERENT places with the
/// same complaint are two mistakes, and both survive. The key is
/// `(severity, message, span)` precisely so this does not collapse.
#[test]
fn one_message_at_two_spans_is_two_mistakes() {
    let program = "\
type { Person } := io.shex(\"p.shex\")

A := io.nonesuch(\"a.xyz\")
B := io.nonesuch(\"b.xyz\")

People : Person from A
    @subject = \"https://example.org/person/{A.id}\"
    name = A.name
";
    let (db, file) = test_support::db_with_document_at("t.fossil", program, "p.shex", SHAPE);
    let spans: Vec<_> = program_diagnostics(&db, file)
        .into_iter()
        .filter(|d| d.message.contains("nonesuch"))
        .map(|d| d.span)
        .collect();
    let unique: std::collections::HashSet<_> = spans.iter().collect();
    assert_eq!(
        spans.len(),
        unique.len(),
        "the dedup must key on the span too, or two mistakes become one"
    );
    assert!(
        spans.len() >= 2,
        "two bindings, two complaints; got {spans:?}"
    );
}

/// The lowering's diagnostics ride out too, and rebased. `check` drains from
/// `lower_to_mir_pg` and not from `typecheck_mapping` for this reason: draining
/// the typechecker alone reported `ok` for programs `run` then refused.
///
/// The span assertion is what makes it a rebase test and not a presence test: a
/// mapping-relative span would point at the top of the file.
#[test]
fn a_mapping_diagnostic_comes_out_rebased_onto_the_file() {
    let program = "\
type { Person } := io.shex(\"p.shex\")

Users := io.csv(\"users.csv\")

People : Person from Nowhere
    @subject = \"https://example.org/person/{Users.id}\"
    name = Users.name
";
    let (db, file) = test_support::db_with_document_at("t.fossil", program, "p.shex", SHAPE);
    let at = u32::try_from(program.find("Nowhere").expect("fixture")).expect("fixture fits");
    let diagnostics = program_diagnostics(&db, file);
    assert!(
        diagnostics.iter().any(|d| d.span.start >= at),
        "a diagnostic about `from Nowhere` must land at or after byte {at} of \
         the file, not at a mapping-relative offset; got {:?}",
        diagnostics
            .iter()
            .map(|d| (d.span, &d.message))
            .collect::<Vec<_>>()
    );
}

/// A clean program is clean — the guard against a drain that reports something
/// for everything, which would satisfy every test above and be useless.
#[test]
fn a_program_with_nothing_wrong_reports_nothing() {
    let program = "\
type { Person } := io.shex(\"p.shex\")

Users := io.csv(\"users.csv\")

People : Person from Users
    @subject = \"https://example.org/person/{Users.id}\"
    name = Users.name
";
    let (db, file) = test_support::db_with_document_at("t.fossil", program, "p.shex", SHAPE);
    let errors: Vec<_> = program_diagnostics(&db, file)
        .into_iter()
        .filter(|d| d.severity == fossil_base::Severity::Error)
        .map(|d| d.message)
        .collect();
    assert!(errors.is_empty(), "a clean program reported {errors:?}");
}
