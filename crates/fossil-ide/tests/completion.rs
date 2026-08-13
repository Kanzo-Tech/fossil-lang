//! SC#4 (LSP-01 completion half) — three-source completion integration test.
//!
//! Proves [`fossil_ide::completions`] merges the three SC#4 sources end-to-end:
//!
//!   (a) a stdlib completion for an un-imported namespace carries a non-empty
//!       `additional_text_edits` (the gleam-lsp auto-import edit);
//!   (b) a `NativeUdfOnly` entry is tagged so the playground can gray it out;
//!   (c) a declared prefix appears as a completion;
//!   (d) when the cursor's mapping resolves a target `ShEx` shape, the shape's
//!       predicate names appear as completions.
//!
//! Case (d) needs a program that NAMES its output shape document and
//! a HOST that has done its two jobs for it: installed a decoder row that
//! claims `.shex`, and REGISTERED the document as a Salsa input before the
//! query asks for it. That is what `HostDb::new` + [`file`] do here, and it is
//! what `fossil-lsp` and `fossil-wasm` do in production. A host that skips
//! either one resolves no shape — which is the correct answer, not a bug, and
//! is why case (d) would otherwise fail silently.
//! Cases (a)-(c) only need the stdlib catalog + the cross-file prefix index,
//! which any db provides.

#![cfg(not(target_arch = "wasm32"))]
// The `.fossil` fixtures contain `${ex:}` / `${.id}` template placeholders —
// LITERAL Fossil source, not Rust format-string args.
#![allow(clippy::literal_string_with_formatting_args)]

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::{Files, FsError, Provider, SourceFile, System};
use lsp_types::{CompletionItemKind, CompletionItemTag};

/// The test's host `System`: a filesystem plus the `ShEx` decoder row, exactly
/// as `fossil-lsp`'s `LspSystem` installs it. `fossil_base::NativeSystem` is
/// not enough any more — its decoder table is the trait default `&[]`, so a
/// `.shex` it can read is still a document nothing decodes.
#[derive(Debug, Default)]
struct HostSystem;

impl System for HostSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| FsError::Io(e.to_string()))
    }
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}

/// A host db stand-in: a real Salsa db over a `HostSystem`, so the shape
/// document the program names is readable from disk.
#[salsa::db]
#[derive(Clone)]
struct HostDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
}

impl std::fmt::Debug for HostDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDb").finish_non_exhaustive()
    }
}

#[salsa::db]
impl salsa::Database for HostDb {}

#[salsa::db]
impl fossil_base::Db for HostDb {
    fn system(&self) -> &dyn System {
        &*self.system
    }
    fn files(&self) -> &Files {
        &self.files
    }
}

impl HostDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            system: Arc::new(HostSystem),
            files: Files::default(),
        }
    }
}

/// The program NAMES its output shape document — `tests/fixtures/person.shex`,
/// which declares `ex:Person` with an `ex:name` triple constraint narrowed to
/// `xsd:integer`. `resolve_target_shape` reads it, so the
/// completion path resolves a shape here for the same reason the compiler does,
/// and stops resolving one when the program stops asking.
///
/// The line numbers below are load-bearing for the cursor positions in these
/// tests: the mapping body is line 3 now, not line 2.
const SRC: &str = "\
prefix ex: <http://example.org/>
type { Person } = io.shex(\"tests/fixtures/person.shex\")
User : ex:Person from users
    ex:name = .name
";

/// Intern the program AND register the documents it names — the host's half,
/// spelled here with the same function `fossil-lsp` and `fossil-wasm` call.
fn file(db: &mut HostDb, src: &str) -> SourceFile {
    let f = SourceFile::new(db, src.to_string(), "complete.fossil".to_string());
    fossil_ide::register_missing_documents(db, f, &|key| std::fs::read_to_string(key).ok());
    f
}

/// (a) A stdlib completion for an un-imported namespace carries a non-empty
///     auto-import `additional_text_edits` (the gleam-lsp pattern).
#[test]
fn stdlib_completion_for_unimported_namespace_has_auto_import_edit() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 3, 14);

    let trim = items
        .iter()
        .find(|i| i.label == "clean.trim")
        .expect("clean.trim must be offered");
    assert_eq!(trim.kind, Some(CompletionItemKind::FUNCTION));
    let edits = trim
        .additional_text_edits
        .as_ref()
        .expect("an un-imported namespace must carry an auto-import edit");
    assert!(!edits.is_empty(), "auto-import edit must be non-empty");
    assert!(
        edits[0].new_text.contains("use clean"),
        "the auto-import edit must insert `use clean`; got {:?}",
        edits[0].new_text,
    );
}

/// (b) A `NativeUdfOnly` entry is tagged DEPRECATED so the playground can gray
///     it out.
#[test]
fn native_only_stdlib_entry_is_tagged() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 3, 14);

    // `clean.slug` lowers to a Rust UDF → NativeUdfOnly.
    let slug = items
        .iter()
        .find(|i| i.label == "clean.slug")
        .expect("clean.slug must be offered");
    assert_eq!(
        slug.tags.as_deref(),
        Some(&[CompletionItemTag::DEPRECATED][..]),
        "a NativeUdfOnly entry must be tagged",
    );
}

/// (c) A declared prefix appears as a completion.
#[test]
fn declared_prefix_is_offered() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 3, 14);

    assert!(
        items.iter().any(|i| i.label == "ex:"),
        "the declared `ex` prefix must be offered; labels = {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// (d) When the cursor's mapping resolves a target `ShEx` shape, the shape's
///     predicate names appear as Field completions.
#[test]
fn shape_property_names_are_offered_when_shape_resolves() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    // Line 3 (`    ex:name = .name`) is inside the `User : ex:Person` mapping
    // whose target shape resolves to `ex:Person`; column 14 is inside the body.
    let items = fossil_ide::completions(&db, &[f], f, 3, 14);

    let shape_prop = items
        .iter()
        .find(|i| i.kind == Some(CompletionItemKind::FIELD))
        .expect("a resolved target shape must contribute Field (shape-property) completions");
    assert_eq!(
        shape_prop.label, "http://example.org/name",
        "the shape's `ex:name` predicate IRI must be offered as a property; got {:?}",
        shape_prop.label,
    );
    // Risk Register: the rendered detail must not leak internal type state.
    let detail = shape_prop.detail.as_deref().unwrap_or("");
    assert!(
        !detail.contains("Unknown") && !detail.contains("InferenceId"),
        "shape-property detail leaked internal type state; got {detail:?}",
    );
}

/// A program that names NO document contributes no shape properties, while
/// stdlib and prefixes still do — the "if reachable" hedge.
///
/// What turns backward checking off is the program declaring no output
/// contract; there is nowhere else for that condition to come from any more.
#[test]
fn a_program_naming_no_document_yields_no_shape_properties_but_keeps_stdlib() {
    const NO_DOCUMENT: &str = "\
prefix ex: <http://example.org/>
User : ex:Person from users
    ex:name = .name
";
    let mut db = HostDb::new();
    let f = file(&mut db, NO_DOCUMENT);
    // One line shorter than `SRC` — the body is line 2 here.
    let items = fossil_ide::completions(&db, &[f], f, 2, 14);

    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
        "a program with no output contract must contribute no shape-property \
         (Field) completions",
    );
    assert!(
        items.iter().any(|i| i.label == "clean.trim"),
        "stdlib completions must still be offered",
    );
}
