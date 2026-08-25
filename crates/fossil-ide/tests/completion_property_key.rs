//! Where a property KEY can go, and what it is spelled.
//!
//! The shape-property source is the one that offers a mapping's output
//! contract, and it had two independent defects. Both are measured here against
//! `tests/fixtures/shop.shex` — `shop:Person` declaring `shop:email` and
//! `shop:name` — with the same host `fossil-lsp` installs.
//!
//! **The label.** It offered `constraint.predicate`, the raw predicate IRI. A
//! property key is never the IRI: it is
//! [`fossil_graph_schema::short_name`] of it — the `@rename` the program
//! addressed to that predicate, else the last segment. Accepting
//! `https://shop.example/voc#email` writes a line the parser refuses
//! (`PropertyLhs := IDENT`), and accepting `name` where the program wrote
//! `@rename(Person, "…#name" as full_name)` writes one the checker refuses.
//!
//! **The position.** It fired wherever the cursor was inside a mapping whose
//! shape resolved — the header, the right-hand side of `=`, and after a member
//! access. Measured before, against the `shop` corpus program
//! (`apps/docs/programs/shop`, whose `Person` has a third predicate `shop:phone`):
//!
//! | position                       | total | FUNCTION | FIELD              |
//! |--------------------------------|-------|----------|--------------------|
//! | key position (end of `email`)  | 54    | 51       | 3 raw IRIs         |
//! | after `User.`                  | 16    | 13       | the SAME 3 raw IRIs|
//! | top level, outside the mapping | 51    | 51       | 0                  |
//!
//! and against this file's two-predicate fixture: 53 / 51 / 2 at the key
//! position, at the header, on the right of `=` and on a blank body line, and
//! 15 / 13 / 2 after `User.`.
//!
//! # What these tests do NOT prove
//!
//! - **That the stdlib source knows where it is.** 51 dotted function names are
//!   still offered in key position, and `str.trim` is no more writable there
//!   than a raw IRI was. That is a third defect, measured
//!   ([`the_catalogue_is_still_offered_in_key_position`] pins it) and not fixed:
//!   it is the stdlib source's [`fossil_ide`] `Scope::Catalogue` arm, a separate
//!   measurement and a separate commit.
//! - **That a name collision is handled.** The table comes from
//!   `ResolvedShape::short_names`, which drops the second of two predicates
//!   sharing a short name; no fixture here declares one, so that path is
//!   untested from this side.
//! - **Anything about a real editor.** Every position below is a `(line,
//!   character)` this file computes; no LSP client is driven.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::{Catalogue, Files, FsError, Provider, SourceFile, System};
use lsp_types::CompletionItemKind;

/// The host's two jobs: read the document off disk, and install the decoder row
/// that claims `.shex`. `NativeSystem`'s decoder table is the trait default
/// `&[]`, so a `.shex` it can read is a document nothing decodes.
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

fn host() -> HostDb {
    HostDb {
        storage: salsa::Storage::default(),
        system: Arc::new(HostSystem),
        files: Files::default(),
        catalogue: Catalogue::default(),
    }
}

/// Intern `src` under a path NEXT TO the fixture document, so `io.shex("shop.shex")`
/// resolves the way it does for a program on disk, and register what it names.
fn program(db: &mut HostDb, src: &str) -> SourceFile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join("property-key.fossil");
    let f = SourceFile::new(&*db, src.to_string(), path.to_string_lossy().into_owned());
    fossil_ide::register_missing_documents(db, f, &|key| std::fs::read_to_string(key).ok());
    f
}

/// The labels of one kind, in the order completion emitted them.
fn labels(items: &[lsp_types::CompletionItem], kind: CompletionItemKind) -> Vec<&str> {
    items
        .iter()
        .filter(|i| i.kind == Some(kind))
        .map(|i| i.label.as_str())
        .collect()
}

fn fields(db: &HostDb, f: SourceFile, line: u32, character: u32) -> Vec<String> {
    fossil_ide::completions(db, &[f], f, line, character)
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
        .map(|i| i.label.clone())
        .collect()
}

/// Line 3 is `    email = User.email`, and every column below is read off it:
/// `email` (the KEY) is columns 4–8, so 9 is the boundary an editor puts the
/// cursor at after the last keystroke of the name; `=` is at 10; `User` is
/// 12–15 and its `.` is at 16.
const SHOP: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")
Users : Person from User
    email = User.email
";

/// The same program with the repair the compiler recommends for a colliding
/// predicate, addressed to `shop:name`.
const RENAMED: &str = "\
@rename(Person, \"https://shop.example/voc#name\" as full_name)
type { Person, Order } := io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")
Users : Person from User
    email = User.email
";

/// A fresh, still-empty line inside the body — four spaces and nothing else,
/// which is what an editor leaves after `Enter` on an indented line.
///
/// The indent is written `\x20` because it is load-bearing and trailing
/// whitespace does not survive being looked at: with the four spaces gone,
/// `(4, 4)` is a column past the end of an empty line, and
/// `position_to_offset` hands `token_at_position` an offset past the CST's
/// range, which panics inside `rowan` («Bad offset: range 0..124 offset 127»).
/// That is a live defect in `crate::position`'s UTF-16 conversion and not this
/// file's subject; it is recorded here because this fixture is the shortest
/// reproducer anyone will have.
const BLANK: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")
Users : Person from User
    email = User.email
\x20\x20\x20\x20
";

/// The identity line, which the parser gives a `PROPERTY_LHS` of its own — the
/// same node a key gets, holding an `AT_ATTR` instead of an `IDENT`.
const SUBJECT: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")
Users : Person from User
    @subject = \"https://shop.example/user/{User.email}\"
    email = User.email
";

/// No `type` binding, so no shape resolves and there is no output contract at
/// all.
const NO_SHAPE: &str = "\
User := io.csv(\"users.csv\")
Users : Person from User
    email = User.email
";

/// **The label.** In key position the shape's predicates are offered under the
/// name a program WRITES — the last segment — and the IRI is not a label.
///
/// Before: `["https://shop.example/voc#email", "https://shop.example/voc#name"]`.
#[test]
fn the_key_position_offers_the_name_a_program_writes() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    assert_eq!(
        fields(&db, f, 3, 9),
        vec!["email", "name"],
        "a property key is the last segment, in the order the document declares them",
    );
    // The cursor mid-name, which is what an editor re-requesting on every
    // keystroke actually sends.
    assert_eq!(fields(&db, f, 3, 6), vec!["email", "name"]);
}

/// **The label, and the half that is silent.** `local_name` and `short_name`
/// agree on every predicate no `@rename` names, so a fixture without one cannot
/// tell them apart. This is the one that can: the program renames
/// `shop:name`, and `name` is a key the checker refuses.
#[test]
fn a_renamed_predicate_is_offered_under_its_rename() {
    let mut db = host();
    let f = program(&mut db, RENAMED);
    assert_eq!(
        fields(&db, f, 4, 9),
        vec!["email", "full_name"],
        "`@rename` is what makes the key writable, so it is what the key is",
    );
}

/// **The position.** After a member access the receiver's members are the
/// answer, and a property key is not one of them.
///
/// Before: the same 2 raw IRIs, beside the 13 relation verbs.
#[test]
fn a_member_access_offers_no_property_key() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    assert!(
        fields(&db, f, 3, 17).is_empty(),
        "`User.` is a member position; got {:?}",
        fields(&db, f, 3, 17),
    );
    assert_eq!(
        labels(
            &fossil_ide::completions(&db, &[f], f, 3, 17),
            CompletionItemKind::FUNCTION
        )
        .len(),
        13,
        "the relation verbs are untouched by this",
    );
}

/// **The position.** The right of `=` is an expression, and a property key is
/// not an expression.
#[test]
fn an_expression_position_offers_no_property_key() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    assert!(
        fields(&db, f, 3, 16).is_empty(),
        "`User` on the right of `=` is a value; got {:?}",
        fields(&db, f, 3, 16),
    );
}

/// **The position.** The header names the shape and the row; no key goes there.
#[test]
fn a_mapping_header_offers_no_property_key() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    assert!(
        fields(&db, f, 2, 3).is_empty(),
        "`Users : Person from User` is the header; got {:?}",
        fields(&db, f, 2, 3),
    );
}

/// **The position.** Outside every mapping there is no shape to draw from —
/// this one was already right, and is pinned so the widening it forbids stays
/// forbidden.
#[test]
fn outside_any_mapping_offers_no_property_key() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    assert!(fields(&db, f, 1, 4).is_empty());
}

/// A mapping whose shape does not resolve contributes nothing, in key position
/// like everywhere else. This is what keeps the tests above from passing
/// vacuously in the other direction — they are non-empty because the shape
/// resolves, and this one is empty because it does not.
#[test]
fn a_mapping_whose_shape_does_not_resolve_offers_no_property_key() {
    let mut db = host();
    let f = program(&mut db, NO_SHAPE);
    assert!(
        fields(&db, f, 2, 9).is_empty(),
        "no `type` binding, no output contract; got {:?}",
        fields(&db, f, 2, 9),
    );
}

/// **The position, and the one the node kind alone would get wrong.**
/// `@subject` is `SubjectAssign := AT_ATTR ASSIGN Expression`, a different
/// production that shares the `PROPERTY_LHS` node with
/// `Property := PropertyLhs ASSIGN Expression`. A key cannot be written where
/// it stands, so the gate is the TOKEN — `PropertyLhs := IDENT` — and not the
/// parent node on its own.
///
/// Before: the same 2 raw IRIs, at both columns.
#[test]
fn the_identity_line_is_not_a_key_position() {
    let mut db = host();
    let f = program(&mut db, SUBJECT);
    // `@subject` is columns 4–11; 12 is the boundary past its last character
    // and 8 is inside it.
    for character in [8, 12] {
        assert!(
            fields(&db, f, 3, character).is_empty(),
            "`@subject` is the identity, not a key; got {:?} at column {character}",
            fields(&db, f, 3, character),
        );
    }
    // …and the property line below it still is one, so this is a distinction
    // and not silence.
    assert_eq!(fields(&db, f, 4, 9), vec!["email", "name"]);
}

/// **The boundary, pinned rather than hoped.** A body line with nothing on it
/// yet is a position where a key COULD go, and it is not resolved: the parser
/// attaches a fresh line's indent to the PREVIOUS property's expression, so the
/// CST cannot tell it from a value continued onto a second line. Offering
/// nothing is the answer until it can.
///
/// The first keystroke closes it — one character makes a `PROPERTY_LHS`, which
/// [`the_key_position_offers_the_name_a_program_writes`] measures.
///
/// Before: 2 raw IRIs here too.
#[test]
fn a_blank_body_line_is_not_a_position_this_resolves() {
    let mut db = host();
    let f = program(&mut db, BLANK);
    assert!(
        fields(&db, f, 4, 4).is_empty(),
        "a fresh body line is indistinguishable from a continued value; got {:?}",
        fields(&db, f, 4, 4),
    );
}

/// The predicate IRI stays REACHABLE — as the item's detail, the way the
/// stdlib source keeps `str.trim` reachable behind the label `trim`. Moving it
/// out of the label must not delete it.
#[test]
fn the_detail_keeps_the_predicate_iri_and_leaks_no_type_state() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    let items = fossil_ide::completions(&db, &[f], f, 3, 9);
    let email = items
        .iter()
        .find(|i| i.label == "email")
        .expect("`email` is offered in key position");
    let detail = email.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("https://shop.example/voc#email"),
        "the IRI is what the key MEANS and has to stay readable; got {detail:?}",
    );
    assert!(
        !detail.contains("Unknown") && !detail.contains("InferenceId"),
        "completion detail leaked internal type state; got {detail:?}",
    );
}

/// **Not fixed, and measured so.** The stdlib source is position-blind: it
/// offers the whole catalogue, dotted, in key position — 51 items, none of
/// which parses as a `PropertyLhs` either. Narrowing it is the same argument
/// this file makes about the shape source and a different measurement, so it is
/// pinned as-is rather than changed.
#[test]
fn the_catalogue_is_still_offered_in_key_position() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    let items = fossil_ide::completions(&db, &[f], f, 3, 9);
    let fns = labels(&items, CompletionItemKind::FUNCTION);
    assert_eq!(fns.len(), 51, "the whole catalogue, dotted; got {fns:?}");
    assert!(fns.contains(&"str.trim"));
}
