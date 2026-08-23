//! **Ruling 13, end to end through the real host.** One registry, dispatch by
//! name, and the two diagnostics a mismatch produces.
//!
//! `fossil-engine` is the crate with the WHOLE host: it installs
//! `fossil_descriptors_output::PROVIDERS` (`src/system.rs`) and registers the
//! documents a program names (`src/documents.rs`), so a `.ttl` sitting on disk
//! beside the program is enough. In any crate below it the document has to be
//! pushed into Salsa by hand, and a harness that registered its own documents
//! would be testing its own registration.
//!
//! # What each test here is evidence for
//!
//! - `catalogue` is the conformance program whose type document is **not**
//!   `ShEx`, and it exists to prove the shape-document seam is not ShEx-specific
//!   (`SURFACE-PLAN.md` ruling 13). It was the only one of the eighteen failing
//!   for that cause: nothing decoded SHACL, so `Product` bound no shape, and by
//!   ruling 3 of 2026-08-11 no property in that mapping was writable.
//! - The two mismatches were **unexpressible** before, not merely unreported.
//!   `def_map` threw the constructor away and every dispatch went by extension,
//!   so `io.shex("x.ttl")` and `io.shacl("x.ttl")` were the same program and
//!   `type { P } := io.csv(…)` asked a question the table had no shape for.
//! - The run reads its shape document through the same rows the checker does,
//!   so a `ShExC` document no longer type-checks and then fails the run.

// The `.fossil` sources below carry `"…{User.id}…"` interpolation holes —
// LITERAL fossil source, not Rust format-string args. Same allow, same reason,
// as `fossil-hir`'s `shapes.rs` and the two `fossil-ide` integration tests.
#![allow(clippy::literal_string_with_formatting_args)]

use std::path::{Path, PathBuf};

/// `apps/docs/programs/` — the conformance set, on disk.
fn programs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/docs/programs")
        .canonicalize()
        .expect("apps/docs/programs is on disk")
}

/// Every diagnostic message `fossil check` produces for `path`.
fn messages(path: &Path) -> Vec<String> {
    introspect(path);
    fossil_engine::check(path)
        .expect("the program is readable")
        .diagnostics
        .into_iter()
        .map(|d| d.message)
        .collect()
}

/// A program (and its neighbours) written into a tempdir, checked.
fn check_program(files: &[(&str, &str)]) -> Vec<String> {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).expect("write");
    }
    messages(&dir.path().join(files[0].0))
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
/// including the two that read types, which lived in a second table keyed by a
/// different criterion.
#[test]
fn one_table_carries_every_constructor() {
    let listed: Vec<String> = fossil_engine::providers()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(
        listed,
        ["csv", "json", "parquet", "rdf", "shacl", "shex"],
        "`fossil providers` lists the whole registry, sorted"
    );
}

/// `ProviderKind` has carried `Schema` since it was written with nothing ever
/// producing one — the wire contract was shaped for capabilities and the data
/// behind it was half a table.
#[test]
fn the_wire_contract_finally_reports_a_schema_provider() {
    let listed = fossil_engine::providers();
    let kind = |name: &str| {
        listed
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name}"))
            .kind
    };
    assert_eq!(kind("csv"), fossil_lineage::ProviderKind::Data);
    assert_eq!(kind("shex"), fossil_lineage::ProviderKind::Schema);
    assert_eq!(kind("shacl"), fossil_lineage::ProviderKind::Schema);
}

// ------------------------------------------------------------- the SHACL row

/// **The criterion for the SHACL row.** `catalogue` is the one conformance
/// program whose type document is SHACL, and it exists to prove the seam is not
/// ShEx-specific. What must be gone is the `SHAPE` cause: `Product` bound no
/// shape because nothing decoded SHACL.
#[test]
fn catalogue_resolves_its_shacl_document() {
    let path = programs_dir().join("catalogue/catalogue.fossil");
    let diagnostics = messages(&path);
    for evidence in [
        "names no shape document",
        "nothing here reads",
        "is not a provider",
        "reads rows, not types",
    ] {
        assert!(
            !diagnostics.iter().any(|m| m.contains(evidence)),
            "the SHACL document must resolve; got {diagnostics:?}"
        );
    }
}

/// Not vacuous: the shape really carries the two predicates the program writes,
/// and their short names are what the bare keys resolve against.
#[test]
fn the_shacl_document_decodes_to_the_predicates_the_program_writes() {
    let ttl = std::fs::read_to_string(programs_dir().join("catalogue/catalogue.ttl"))
        .expect("the document is on disk");
    let shapes =
        fossil_descriptors_output::decode_shacl("catalogue.ttl", &ttl).expect("the row decodes it");
    let product = shapes
        .lookup("https://shop.example/voc#Product")
        .expect("`sh:targetClass shop:Product` is the shape's IRI");
    let names: Vec<&str> = product
        .properties
        .iter()
        .map(|c| fossil_graph_schema::local_name(&c.predicate))
        .collect();
    assert_eq!(names, ["label", "price"], "the keys `catalogue` writes");
}

// -------------------------------------------------- the two new diagnostics

/// **Asking a row for a capability it does not declare names both.** This was
/// not a wrong answer before, it was no question: the data table and the schema
/// table had no shared vocabulary in which to ask it.
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
        diagnostics
            .iter()
            .any(|m| m == "`io.csv` reads rows, not types — `io.shex`, `io.shacl` read types"),
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
        diagnostics.iter().any(|m| m.starts_with(
            "`io.shex` reads types, not rows — `io.csv`, `io.json`, `io.parquet`, `io.rdf`"
        )),
        "got {diagnostics:?}"
    );
}

/// **The row checks its own extension and words its own rejection**, naming the
/// constructor and the extension. `io.shex("catalogue.ttl")` used to be
/// byte-identical in behaviour to `io.shacl("catalogue.ttl")`, because the
/// extension picked the row and the name was decorative.
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
        diagnostics.iter().any(|m| m
            == "`io.shex` reads `.shex`, `.shexj` or `.shexc` documents, and \
                `person.ttl` is `.ttl`"),
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
        diagnostics
            .iter()
            .any(|m| m.starts_with("`io.linkml` is not a provider this host installs")),
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
        !diagnostics.iter().any(|m| m.contains("is not a provider")),
        "got {diagnostics:?}"
    );
}

// ------------------------------------------------- `schema =` names a provider

/// **The `schema =` argument carries its own row.** It was the last position
/// where a document arrived without one, and its decoder came from the
/// document's extension — the one dispatch left that did not go by name.
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
    for evidence in [
        "named by no provider",
        "is not a provider",
        "reads rows, not types",
    ] {
        assert!(
            !diagnostics.iter().any(|m| m.contains(evidence)),
            "the argument names `io.shex` and it reads types; got {diagnostics:?}"
        );
    }
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
        diagnostics.iter().any(|m| m
            == "`schema =` names a document, and a document is named by the provider \
                that reads it: write `schema = io.shex(\"…\")` or \
                `schema = io.shacl(\"…\")`, never a bare path — the row that reads a \
                document is the one the program names, not the one its extension \
                happens to match"),
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
        diagnostics
            .iter()
            .any(|m| m == "`io.csv` reads rows, not types — `io.shex`, `io.shacl` read types"),
        "got {diagnostics:?}"
    );
}

// ------------------------------------- once per mistake, and never zero times

/// **A binding's diagnostic is reported ONCE**, whatever the mapping count.
/// Salsa accumulates over a query's whole dependency subtree and `lower_to_hir`
/// is in every mapping's, so one mistake in a three-mapping program came out
/// three times — and there is no mapping it belongs to.
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
        .filter(|m| m.starts_with("`io.csv` reads rows, not types"))
        .count();
    assert_eq!(hits, 1, "three mappings, one mistake; got {diagnostics:?}");
}

/// **And never zero.** A file with no mapping used to lose it entirely: `check`
/// fell back to draining `def_map`, and `lower_to_hir` is not in its subtree. A
/// program that is only bindings is exactly the program a provider mistake is
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
            .any(|m| m.starts_with("`io.csv` reads rows, not types")),
        "got {diagnostics:?}"
    );
}

/// Introspect before compiling — what `fossil-cli` does, and what `check`/`run`
/// stopped doing for themselves. Without it a program's sources have no
/// forward-propagated types, which is a different (and quietly weaker) answer.
fn introspect(path: &std::path::Path) {
    let system = fossil_engine::host_system(path);
    let _ = fossil_introspect::introspect_program(
        &*system,
        path,
        &std::collections::HashMap::new(),
        &fossil_introspect::RunCreds::default(),
    );
}
