//! SC#4 (LSP-01 completion half) — completion integration test.
//!
//! Proves [`fossil_ide::completions`] merges its sources end-to-end:
//!
//!   (a) when the cursor's mapping resolves a target `ShEx` shape, the shape's
//!       predicate names appear as `Field` completions;
//!   (b) a program that names no document contributes none of those, while the
//!       stdlib catalogue still answers — the "if reachable" hedge.
//!
//! Three cases stood in front of (a) and all three named something that no
//! longer exists: a `clean.trim` whose completion carried a top-of-file
//! `use clean` auto-import edit, a `clean.slug` tagged `DEPRECATED` for being
//! `NativeUdfOnly`, and a declared `ex:` prefix offered as its own item. There
//! is no `use` production, no `WasmClass::NativeUdfOnly`, and no `prefix`
//! declaration; the catalogue is `str.*` / `io.*` / `seq.*` and every row of it
//! runs everywhere. `completion.rs`'s own `stdlib_entries_are_offered_bare` is
//! what asserts that now, and case (b) below is what keeps a stdlib assertion at
//! the integration layer.
//!
//! Case (a) needs a program that NAMES its output shape document and
//! a HOST that has done its two jobs for it: installed a decoder row that
//! claims `.shex`, and REGISTERED the document as a Salsa input before the
//! query asks for it. That is what `HostDb::new` + [`file`] do here, and it is
//! what `fossil-lsp` and `fossil-wasm` do in production. A host that skips
//! either one resolves no shape — which is the correct answer, not a bug, and
//! is why case (a) would otherwise fail silently.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::{Catalogue, Files, FsError, Provider, SourceFile, System};
use lsp_types::CompletionItemKind;

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
    catalogue: Catalogue,
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

    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }
}

impl HostDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            system: Arc::new(HostSystem),
            files: Files::default(),
            catalogue: Catalogue::default(),
        }
    }
}

/// The program NAMES its output shape document — `tests/fixtures/person.shex`,
/// which declares `http://example.org/Person` with an `http://example.org/name`
/// triple constraint narrowed to `xsd:integer`. `resolve_target_shape` reads it,
/// so the completion path resolves a shape here for the same reason the compiler
/// does, and stops resolving one when the program stops asking.
///
/// The line numbers below are load-bearing for the cursor positions in these
/// tests: the mapping body is line 3.
const SRC: &str = "\
type { Person } := io.shex(\"tests/fixtures/person.shex\")
users := io.csv(\"users.csv\")
User : Person from users
    name = users.name
";

/// Intern the program AND register the documents it names — the host's half,
/// spelled here with the same function `fossil-lsp` and `fossil-wasm` call.
fn file(db: &mut HostDb, src: &str) -> SourceFile {
    let f = SourceFile::new(db, src.to_string(), "complete.fossil".to_string());
    fossil_ide::register_missing_documents(db, f, &|key| std::fs::read_to_string(key).ok());
    f
}

/// (a) When the cursor's mapping resolves a target `ShEx` shape, the shape's
///     predicate names appear as Field completions.
#[test]
fn shape_property_names_are_offered_when_shape_resolves() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    // Line 3 (`    name = users.name`) is inside the `User : Person` mapping
    // whose target shape resolves to `http://example.org/Person`; column 14 is
    // inside the body and is not the `.` that would trigger source fields.
    let items = fossil_ide::completions(&db, &[f], f, 3, 14);

    let shape_prop = items
        .iter()
        .find(|i| i.kind == Some(CompletionItemKind::FIELD))
        .expect("a resolved target shape must contribute Field (shape-property) completions");
    assert_eq!(
        shape_prop.label, "http://example.org/name",
        "the shape's `name` predicate IRI must be offered as a property; got {:?}",
        shape_prop.label,
    );
    // Risk Register: the rendered detail must not leak internal type state.
    let detail = shape_prop.detail.as_deref().unwrap_or("");
    assert!(
        !detail.contains("Unknown") && !detail.contains("InferenceId"),
        "shape-property detail leaked internal type state; got {detail:?}",
    );
}

/// (b) A program that names NO document contributes no shape properties, while
///     the stdlib catalogue still does — the "if reachable" hedge.
///
/// What turns backward checking off is the program declaring no output
/// contract; there is nowhere else for that condition to come from any more.
///
/// This is also the one integration-level assertion left that the stdlib source
/// answers at all: the three tests that used to make it did so about entries
/// (`clean.trim`, `clean.slug`) and an item kind (`ex:`) the catalogue no longer
/// has. `str.trim` is a row it does have.
#[test]
fn a_program_naming_no_document_yields_no_shape_properties_but_keeps_stdlib() {
    const NO_DOCUMENT: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.name
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
        items.iter().any(|i| i.label == "str.trim"),
        "stdlib completions must still be offered; labels = {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
    assert!(
        !items.iter().any(|i| i.label.ends_with(':')),
        "a `prefix:` completion is a form this language does not have",
    );
}
