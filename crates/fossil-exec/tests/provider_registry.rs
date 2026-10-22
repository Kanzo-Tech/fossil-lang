//! **One registry, dispatch by name, end to end through a whole host** — and
//! the two diagnostics a mismatch produces.
//!
//! The check host in `tests/support/native.rs` installs
//! `fossil_hir::providers::PROVIDERS` and registers the documents a program
//! names from disk, so a `.ttl` sitting beside the program is enough; nothing
//! here registers a document by hand, which would be testing its own
//! registration.
//!
//! # What each test here is evidence for
//!
//! - `catalogue` is the conformance program whose type document is **not**
//!   `ShEx`; `tests/programs.rs` runs it, and its empty `diagnostic.txt` and
//!   resolved type binding are what prove the seam is not ShEx-specific.
//! - Both mismatches need dispatch by NAME to be expressible at all: under
//!   dispatch by extension `io.shex("x.ttl")` and `io.shacl("x.ttl")` are one
//!   program, and `type { P } := io.csv(…)` asks a question a row cannot answer.
//! - The run reads its shape document through the same rows the checker does,
//!   so a `ShExC` document cannot type-check and then fail the run.

// The `.fossil` sources below carry `"…{User.id}…"` interpolation holes —
// LITERAL fossil source, not Rust format-string args. Same allow, same reason,
// as `fossil-hir`'s `shapes.rs` and the two `fossil-ide` integration tests.
#![allow(clippy::literal_string_with_formatting_args)]

use std::path::Path;

use fossil_base::Diagnostic;
use fossil_graph_schema::Problem;

#[path = "support/native.rs"]
mod native;
mod support;

/// Every diagnostic a check of `path` produces.
fn diagnostics_of(path: &Path) -> Vec<Diagnostic> {
    native::check(path).diagnostics
}

/// A program (and its neighbours) written into a tempdir, checked.
fn check_program(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let dir = native::write_dir(files);
    diagnostics_of(&dir.path().join(files[0].0))
}

/// A `WrongCapability` refusal of `constructor`, whose help names who can.
fn refuses_capability(d: &Diagnostic, constructor: &str, capability: &str, who: &str) -> bool {
    matches!(&d.problem, Problem::WrongCapability { constructor: c, capability: w }
        if c == constructor && w == capability)
        && d.help.as_deref().is_some_and(|h| h.contains(who))
}

const fn is_provider_refusal(p: &Problem) -> bool {
    matches!(
        p,
        Problem::UnknownProvider { .. }
            | Problem::WrongCapability { .. }
            | Problem::BareDocumentPath { .. }
            | Problem::NoDocument { .. }
    )
}

/// The registry as a host lists it to a connector UI.
fn formats() -> Vec<fossil_hir::lineage::Format> {
    fossil_hir::lineage::formats(fossil_hir::providers::PROVIDERS)
}

/// A real `ShExC` document declaring `ex:Person` with one `ex:name`.
const PERSON_SHEXC: &str = "\
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string
}
";

const USERS_CSV: &str = "id,name\n1,ada\n";

/// A one-mapping program over `users.csv`, with the type binding spelled by the
/// caller — the line under test in every mismatch below.
fn program_binding(type_line: &str) -> String {
    format!(
        "{type_line}\n\
         User := io.csv(\"users.csv\")\n\
         \n\
         Users : Person from User\n    \
         @subject = \"https://example.org/u/{{User.id}}\"\n    \
         name = User.name\n"
    )
}

// ---------------------------------------------------------------- the registry

/// The registry is ONE table and every `io.*` the language has is in it —
/// including the two that read types.
#[test]
fn one_table_carries_every_constructor() {
    let listed: Vec<String> = formats().into_iter().map(|p| p.name).collect();
    assert_eq!(
        listed,
        ["csv", "json", "parquet", "rdf", "shacl", "shex"],
        "the listing is the whole registry, sorted"
    );
}

/// `FormatKind` is read off the row's capabilities, so a row that reads types
/// is listed as `Schema`.
#[test]
fn the_wire_contract_reports_a_schema_provider() {
    let listed = formats();
    let kind = |name: &str| {
        listed
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name}"))
            .kind
    };
    assert_eq!(kind("csv"), fossil_hir::lineage::FormatKind::Data);
    assert_eq!(kind("shex"), fossil_hir::lineage::FormatKind::Schema);
    assert_eq!(kind("shacl"), fossil_hir::lineage::FormatKind::Schema);
}

// ---------------------------------------------- the two capability diagnostics

/// **Asking a row for a capability it does not declare names both.** One table
/// of rows that declare capabilities is what makes the question askable.
#[test]
fn a_type_binding_on_a_data_provider_names_both() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            program_binding("type { Person } := io.csv(\"users.csv\")").as_str(),
        ),
        ("users.csv", USERS_CSV),
    ]);
    assert!(
        diagnostics.iter().any(|d| refuses_capability(
            d,
            "io.csv",
            "read types",
            "`io.shex`, `io.shacl` read types"
        )),
        "got {diagnostics:?}"
    );
}

/// And the converse, which is the same rule read the other way round.
#[test]
fn a_source_binding_on_a_schema_provider_names_both() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            "type { Person } := io.shex(\"person.shex\")\n\
             User := io.shex(\"person.shex\")\n\
             \n\
             Users : Person from User\n    \
             @subject = \"https://example.org/u/{User.id}\"\n    \
             name = User.name\n",
        ),
        ("person.shex", PERSON_SHEXC),
    ]);
    assert!(
        diagnostics.iter().any(|d| refuses_capability(
            d,
            "io.shex",
            "read rows",
            "`io.csv`, `io.json`, `io.parquet`, `io.rdf`"
        )),
        "got {diagnostics:?}"
    );
}

/// **The row checks its own extension and words its own rejection**, naming the
/// constructor and the extension. Were the extension to pick the row,
/// `io.shex("catalogue.ttl")` would be `io.shacl("catalogue.ttl")` and the name
/// decorative.
#[test]
fn a_shape_document_the_named_row_does_not_read_names_both() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            program_binding("type { Person } := io.shex(\"person.ttl\")").as_str(),
        ),
        ("person.ttl", ""),
        ("users.csv", USERS_CSV),
    ]);
    assert!(
        diagnostics.iter().any(|d| matches!(&d.problem,
            Problem::WrongExtension { constructor, document }
                if constructor == "io.shex" && document == "person.ttl")
            && d.help
                .as_deref()
                .is_some_and(|h| h.contains("`.shex`, `.shexj` or `.shexc`"))),
        "got {diagnostics:?}"
    );
}

/// A name no row carries is reported as what it is — not installed — and the
/// message lists what is. `io` is the namespace of providers, and a new one —
/// `io.linkml`, `io.graphql` — arrives as a ROW in that table and never as a
/// rule of the grammar, so the day it exists this message is the only thing
/// that changes.
#[test]
fn an_unknown_constructor_is_reported_with_the_installed_set() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            program_binding("type { Person } := io.linkml(\"person.yaml\")").as_str(),
        ),
        ("person.yaml", ""),
        ("users.csv", USERS_CSV),
    ]);
    assert!(
        diagnostics.iter().any(|d| matches!(&d.problem,
            Problem::UnknownProvider { constructor } if constructor == "io.linkml")
            && d.help.as_deref().is_some_and(|h| h.contains("`io.csv`"))),
        "got {diagnostics:?}"
    );
}

/// A derived binding is NOT a provider mismatch. `Adults := User.where(…)` reads
/// as the dotted callee `User.where` here, and reporting it would put a second,
/// wrong message on a form `fossil-mir`'s `resolve_source` already explains.
#[test]
fn a_derived_binding_is_not_diagnosed_as_a_provider() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            "type { Person } := io.shex(\"person.shex\")\n\
             User := io.csv(\"users.csv\")\n\
             Adults := User.where(User.id > 1)\n\
             \n\
             Users : Person from Adults\n    \
             @subject = \"https://example.org/u/{User.id}\"\n    \
             name = User.name\n",
        ),
        ("person.shex", PERSON_SHEXC),
        ("users.csv", USERS_CSV),
    ]);
    assert!(
        !diagnostics
            .iter()
            .any(|d| matches!(d.problem, Problem::UnknownProvider { .. })),
        "got {diagnostics:?}"
    );
}

// ------------------------------------------------- `schema =` names a provider

/// **The `schema =` argument carries its own row**, so its decoder does not
/// come from the document's extension.
#[test]
fn a_schema_argument_naming_a_provider_resolves_its_shapes() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            "{ Person } := io.rdf(\"g.ttl\", schema = io.shex(\"person.shex\"))\n\
             \n\
             People : Person from Person\n    \
             @subject = \"https://example.org/u/{Person.subject}\"\n    \
             name = Person.name\n",
        ),
        ("person.shex", PERSON_SHEXC),
        ("g.ttl", ""),
    ]);
    assert!(
        !diagnostics.iter().any(|d| is_provider_refusal(&d.problem)),
        "the argument names `io.shex` and it reads types; got {diagnostics:?}"
    );
}

/// A bare path is an error that says what to write. It is not a fallback to the
/// extension: the row that reads a document is the one the program names.
#[test]
fn a_bare_schema_path_is_an_error_that_says_what_to_write() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            "{ Person } := io.rdf(\"g.ttl\", schema = \"person.shex\")\n\
             \n\
             People : Person from Person\n    \
             @subject = \"https://example.org/u/{Person.subject}\"\n    \
             name = Person.name\n",
        ),
        ("person.shex", PERSON_SHEXC),
        ("g.ttl", ""),
    ]);
    assert!(
        diagnostics.iter().any(|d| matches!(&d.problem,
            Problem::BareDocumentPath { document } if document == "person.shex")
            && d.help
                .as_deref()
                .is_some_and(|h| h.contains("schema = io.shex("))),
        "got {diagnostics:?}"
    );
}

/// And the argument is checked for the same two things every other position is:
/// the capability, and the extension.
#[test]
fn a_schema_argument_on_a_data_provider_names_both() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            "{ Person } := io.rdf(\"g.ttl\", schema = io.csv(\"person.csv\"))\n\
             \n\
             People : Person from Person\n    \
             @subject = \"https://example.org/u/{Person.subject}\"\n    \
             name = Person.name\n",
        ),
        ("person.csv", "id\n1\n"),
        ("g.ttl", ""),
    ]);
    assert!(
        diagnostics.iter().any(|d| refuses_capability(
            d,
            "io.csv",
            "read types",
            "`io.shex`, `io.shacl` read types"
        )),
        "got {diagnostics:?}"
    );
}

// ------------------------------------- once per mistake, and never zero times

/// **A binding's diagnostic is reported ONCE**, whatever the mapping count.
/// `lower_to_hir` is read by every mapping, so a collector that reads it per
/// mapping prints one mistake in a three-mapping program three times — and
/// there is no mapping it belongs to.
#[test]
fn a_binding_diagnostic_is_reported_once_not_once_per_mapping() {
    let diagnostics = check_program(&[
        (
            "prog.fossil",
            "type { Person } := io.csv(\"users.csv\")\n\
             User := io.csv(\"users.csv\")\n\
             \n\
             A : Person from User\n    \
             @subject = \"https://example.org/a/{User.id}\"\n    \
             name = User.name\n\
             \n\
             B : Person from User\n    \
             @subject = \"https://example.org/b/{User.id}\"\n    \
             name = User.name\n\
             \n\
             C : Person from User\n    \
             @subject = \"https://example.org/c/{User.id}\"\n    \
             name = User.name\n",
        ),
        ("users.csv", USERS_CSV),
    ]);
    let hits = diagnostics
        .iter()
        .filter(|d| refuses_capability(d, "io.csv", "read types", "read types"))
        .count();
    assert_eq!(hits, 1, "three mappings, one mistake; got {diagnostics:?}");
}

/// **And never zero.** A file with no mapping must not lose it: draining
/// `def_map` alone would not reach `lower_to_hir`, which is not in its subtree.
/// A program that is only bindings is exactly the program a provider mistake is
/// easiest to make in.
#[test]
fn a_file_with_no_mapping_still_reports_its_bindings() {
    let diagnostics = check_program(&[
        ("prog.fossil", "type { Person } := io.csv(\"users.csv\")\n"),
        ("users.csv", USERS_CSV),
    ]);
    assert!(
        diagnostics
            .iter()
            .any(|d| matches!(d.problem, Problem::WrongCapability { .. })),
        "got {diagnostics:?}"
    );
}
