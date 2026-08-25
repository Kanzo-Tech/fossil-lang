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
/// as `fossil-lsp`'s `LspSystem` installs it. `fossil_base::test_support::NativeSystem` is
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
///     predicates appear as Field completions — in key position, under the name
///     a program writes.
#[test]
fn shape_property_names_are_offered_when_shape_resolves() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    // Line 3 (`    name = users.name`) is inside the `User : Person` mapping
    // whose target shape resolves to `http://example.org/Person`. Column 8 is
    // the end of the KEY — the one position a property key can be written. The
    // cursor sat at column 14 instead, inside `users` on the right of the `=`,
    // and the item it found was labelled `http://example.org/name`: an
    // expression position, and a label no `PropertyLhs := IDENT` accepts.
    // `tests/completion_property_key.rs` is the whole of that measurement.
    let items = fossil_ide::completions(&db, &[f], f, 3, 8);

    let shape_prop = items
        .iter()
        .find(|i| i.kind == Some(CompletionItemKind::FIELD))
        .expect("a resolved target shape must contribute Field (shape-property) completions");
    assert_eq!(
        shape_prop.label, "name",
        "a property key is the predicate's short name, never its IRI; got {:?}",
        shape_prop.label,
    );
    let detail = shape_prop.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("http://example.org/name"),
        "the IRI is what the key means and stays readable in the detail; got {detail:?}",
    );
    // Risk Register: the rendered detail must not leak internal type state.
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
    // One line shorter than `SRC` — the body is line 2 here, and column 8 is
    // the same KEY position case (a) asks from. Asking anywhere else would make
    // this pass for the wrong reason: no position but that one offers a shape
    // property at all now, so an expression position is silent whether the
    // document resolves or not.
    let items = fossil_ide::completions(&db, &[f], f, 2, 8);

    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
        "a program with no output contract must contribute no shape-property \
         (Field) completions",
    );
    // The stdlib half is asked one line up, at the header. A key position
    // offers no catalogued row either now — `PropertyLhs := IDENT` and every
    // row is dotted — so asking here would prove the catalogue silent for the
    // narrowing's reason and read as if the document were the reason.
    let header = fossil_ide::completions(&db, &[f], f, 1, 3);
    assert!(
        header.iter().any(|i| i.label == "str.trim"),
        "stdlib completions must still be offered; labels = {:?}",
        header.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
    assert!(
        !header.iter().any(|i| i.label.ends_with(':')),
        "a `prefix:` completion is a form this language does not have",
    );
}

// ── The receiver ───────────────────────────────────────────────────────────
//
// Measured before the narrowing existed, at the `.` of `users.name` in `SRC`:
// **51 stdlib rows** offered whatever the receiver — 13 `seq.*`, 13 `str.*`,
// 3 `io.*`, the rest `parse` / `math` / `validate` / `core` / `anon` — in
// `HashMap` order, which three consecutive runs printed three different
// spellings of. Below: 13, all of them `seq.*`, in one order.

/// After a `:=` binding's dot, the catalogue offers the RELATION VERBS and
/// nothing else — labelled bare, because the member is what is being typed.
#[test]
fn a_relation_receiver_offers_the_verbs_and_no_other_row() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    // `    name = users.name` is line 3; the `.` is at column 16, so 17 is the
    // first character of the member.
    let items = fossil_ide::completions(&db, &[f], f, 3, 17);
    let fns: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FUNCTION))
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(
        fns,
        vec![
            "aggregate",
            "count",
            "distinct",
            "drop",
            "flatten",
            "group_by",
            "join",
            "map",
            "select",
            "sort",
            "take",
            "union",
            "where",
        ],
        "`users` is a relation: its members are the verbs, sorted, bare",
    );
    // The dotted spelling is still reachable — it is the `detail`.
    let where_ = items.iter().find(|i| i.label == "where").expect("where");
    assert!(
        where_
            .detail
            .as_deref()
            .unwrap_or("")
            .starts_with("seq.where("),
        "the detail must still name the row's full dotted signature; got {:?}",
        where_.detail,
    );
}

/// A `str.` head is a TYPE path, and `receiver_of` classifies it — so the list
/// is what a string has, and carries no verb and no `io` row.
#[test]
fn a_scalar_head_offers_only_that_type_s_members() {
    const WITH_STR: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = str.
";
    let mut db = HostDb::new();
    let f = file(&mut db, WITH_STR);
    // `    name = str.` — the `.` is the last character of line 2, column 14.
    let items = fossil_ide::completions(&db, &[f], f, 2, 15);
    let fns: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FUNCTION))
        .map(|i| i.label.as_str())
        .collect();
    assert!(fns.contains(&"trim"), "a string has `trim`; got {fns:?}");
    assert!(fns.contains(&"upper"), "a string has `upper`; got {fns:?}");
    assert!(
        !fns.contains(&"where") && !fns.contains(&"csv"),
        "a string has no verb and no reader; got {fns:?}",
    );
    let mut sorted = fns.clone();
    sorted.sort_unstable();
    assert_eq!(fns, sorted, "the list is ordered by label, always");
}

/// `Receiver::Namespace` is ONE receiver shared by six heads, so `members_of`
/// alone would answer `io.` with `math.abs`. The head narrows it.
#[test]
fn a_namespace_head_offers_only_its_own_namespace() {
    const WITH_IO: &str = "\
users := io.
";
    let mut db = HostDb::new();
    let f = file(&mut db, WITH_IO);
    let items = fossil_ide::completions(&db, &[f], f, 0, 12);
    let fns: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FUNCTION))
        .map(|i| i.label.as_str())
        .collect();
    assert!(fns.contains(&"csv"), "`io.` offers `csv`; got {fns:?}");
    assert!(
        !fns.contains(&"abs") && !fns.contains(&"email"),
        "`math.abs` and `validate.email` are other namespaces; got {fns:?}",
    );
}

/// A head the catalogue does not know and the file does not bind gets nothing
/// from the stdlib source — the editor stays quiet rather than offering 51.
#[test]
fn an_unknown_head_offers_no_catalogue_row() {
    const UNKNOWN: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = orders.
";
    let mut db = HostDb::new();
    let f = file(&mut db, UNKNOWN);
    let items = fossil_ide::completions(&db, &[f], f, 2, 18);
    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FUNCTION)),
        "`orders` names nothing; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// Away from any dot, and outside a property key, the catalogue is offered
/// whole, spelled in full — and SORTED. `FunctionRegistry` is a `HashMap`;
/// without the sort this vector is a different vector on every call.
///
/// It asked at `(3, 6)`, inside the key `name`. That is the one dot-free
/// position where the catalogue is NOT the answer — no dotted row parses as a
/// `PropertyLhs` — so the ordering claim moved to the mapping header, which is
/// dot-free for the reason this test is about.
#[test]
fn the_bare_catalogue_is_whole_and_sorted() {
    let mut db = HostDb::new();
    let f = file(&mut db, SRC);
    // Line 2 is `User : Person from users`; column 3 is inside `User`.
    let items = fossil_ide::completions(&db, &[f], f, 2, 3);
    let fns: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FUNCTION))
        .map(|i| i.label.as_str())
        .collect();
    assert!(
        fns.contains(&"str.trim") && fns.contains(&"seq.where") && fns.contains(&"io.csv"),
        "no receiver is known here, so every row is offered dotted; got {fns:?}",
    );
    let mut sorted = fns.clone();
    sorted.sort_unstable();
    assert_eq!(fns, sorted, "ordered by label, always");
    // The same call twice is the same list. This is what was false.
    let again: Vec<String> = fossil_ide::completions(&db, &[f], f, 2, 3)
        .into_iter()
        .map(|i| i.label)
        .collect();
    assert_eq!(
        again,
        items.into_iter().map(|i| i.label).collect::<Vec<_>>(),
        "two calls, one order",
    );
}
