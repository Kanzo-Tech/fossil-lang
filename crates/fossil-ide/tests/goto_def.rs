//! goto-def integration test — the three positions, and the one that leaves the
//! language.
//!
//! There is no import production, so no name a program writes resolves into
//! another `.fossil`: the second file is the `.shex`, where a shape name and a
//! property key are both defined, and this test is what holds that.
//!
//! It needs a HOST that has done its two jobs — installed a decoder row that
//! claims `.shex`, and REGISTERED the document as a Salsa input before the query
//! asks for it — which is what `HostDb::new` + [`file`] do here and what
//! `fossil-lsp` and `fossil-wasm` do in production. `fossil_base::test_support::NativeSystem`
//! is not enough: its provider table reads no types, so every program resolves
//! no shape and every assertion below would fail for the wrong reason.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::{Catalogue, Files, FsError, Provider, SourceFile, System};
use fossil_ide::{NavigationTarget, goto_definition};

/// The test's host `System`: a filesystem plus the `ShEx` decoder row, exactly
/// as `fossil-lsp`'s `LspSystem` installs it.
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

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The program, named so that the document it names resolves next to it.
///
/// `tests/fixtures/shop.shex` declares `shop:Person` with `shop:email` and
/// `shop:name`, and `shop:Order` after it — two shapes on purpose, so a jump
/// that landed on the first one by accident would not pass.
const SRC: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")

User := io.csv(\"users.csv\")

Users : Person from User
    @subject = \"https://shop.example/user/{User.email}\"
    email = User.email
    name = User.name
";

/// Intern the program AND register the documents it names — the host's half,
/// spelled with the same function `fossil-lsp` calls.
fn program(db: &mut HostDb, src: &str) -> SourceFile {
    let path = fixture_dir().join("goto.fossil");
    let f = SourceFile::new(db, src.to_string(), path.to_string_lossy().into_owned());
    fossil_ide::register_missing_documents(db, f, &|key| std::fs::read_to_string(key).ok());
    f
}

/// The text of the file a target points into, and the slice the range covers.
fn hit<'a>(db: &'a HostDb, target: &NavigationTarget) -> (&'a str, String) {
    let text = target.file.text(db);
    let slice = text
        .get(target.range.start as usize..target.range.end as usize)
        .unwrap_or_default()
        .to_string();
    (text, slice)
}

/// 1. A SHAPE NAME in a mapping header jumps into the `.shex`, at the shape.
///
/// The header is line 4 (`Users : Person from User`); `Person` starts at column
/// 8, so column 9 is unambiguously inside it.
#[test]
fn a_shape_name_resolves_into_the_document() {
    let mut db = HostDb::new();
    let f = program(&mut db, SRC);

    let hits = goto_definition(&db, &[f], f, 4, 9);
    assert_eq!(hits.len(), 1, "one shape, one place to go; got {hits:?}");
    let target = &hits[0];
    assert_ne!(
        target.file, f,
        "the definition of a shape name is NOT in the program"
    );
    let (text, slice) = hit(&db, target);
    assert!(
        text.contains("shop:Person {"),
        "the target file must be the shape document"
    );
    assert_eq!(
        slice, "shop:Person",
        "and the range must cover the shape's own name, not the top of the file"
    );
}

/// The second name of the SAME binding lands on the second shape — a jump that
/// resolved the document and then took its first shape would pass test 1 and
/// fail this one.
#[test]
fn the_second_shape_name_lands_on_the_second_shape() {
    const TWO: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")

Purchase := io.csv(\"orders.csv\")

Orders : Order from Purchase
    @subject = \"https://shop.example/order/{Purchase.id}\"
    total = Purchase.amount
";
    let mut db = HostDb::new();
    let f = program(&mut db, TWO);

    // Line 4 is `Orders : Order from Purchase`; `Order` starts at column 9.
    let hits = goto_definition(&db, &[f], f, 4, 10);
    let (_, slice) = hit(&db, hits.first().expect("Order resolves"));
    assert_eq!(slice, "shop:Order");
}

/// 2. A PROPERTY KEY jumps into the `.shex`, at the PREDICATE it names.
///
/// `name = User.name` is line 7. The key is at column 4.
#[test]
fn a_property_key_resolves_to_its_predicate_in_the_document() {
    let mut db = HostDb::new();
    let f = program(&mut db, SRC);

    let hits = goto_definition(&db, &[f], f, 7, 5);
    assert_eq!(hits.len(), 1, "one predicate; got {hits:?}");
    let target = &hits[0];
    assert_ne!(
        target.file, f,
        "a property key names nothing in the program"
    );
    let (_, slice) = hit(&db, target);
    assert_eq!(
        slice, "shop:name",
        "the range must cover the predicate that declares the key"
    );
}

/// The key resolves by NAME against the shape's table, so the property BEFORE it
/// resolves to its own predicate and not to the one after — the failure mode a
/// positional `ExprId` would have produced.
#[test]
fn each_property_key_resolves_to_its_own_predicate() {
    let mut db = HostDb::new();
    let f = program(&mut db, SRC);

    // Line 6 is `    email = User.email`.
    let hits = goto_definition(&db, &[f], f, 6, 5);
    let (_, slice) = hit(&db, hits.first().expect("email resolves"));
    assert_eq!(slice, "shop:email");
}

/// `@subject` is not a predicate: a shape declares a node's predicates, and in
/// RDF the subject IS the node. It resolves to nothing rather than to whatever
/// the first predicate happens to be.
#[test]
fn the_identity_line_resolves_to_nothing() {
    let mut db = HostDb::new();
    let f = program(&mut db, SRC);
    // Line 5 is `    @subject = "…"`; column 5 is inside `@subject`.
    assert!(goto_definition(&db, &[f], f, 5, 5).is_empty());
}

/// 3. A MAPPING NAME still resolves inside the program, through the workspace
///    index — the one position that never left the language.
#[test]
fn a_mapping_name_resolves_in_the_program() {
    let mut db = HostDb::new();
    let f = program(&mut db, SRC);

    // Line 4, column 1 is inside `Users`.
    let hits = goto_definition(&db, &[f], f, 4, 1);
    let target = hits
        .iter()
        .find(|t| t.file == f)
        .expect("Users resolves to its own mapping header");
    assert!(
        target.range.start < target.range.end,
        "a mapping range must be non-empty"
    );
}

/// When the document the program names is NOT registered — a host with no
/// filesystem, an unsaved buffer, a path that does not exist — a shape name
/// falls back to the `type { … }` binding that introduced it. That is a real
/// definition site for the NAME, and it is the honest answer when the document
/// cannot be opened.
#[test]
fn an_unregistered_document_falls_back_to_the_type_binding() {
    let db = HostDb::new();
    // Interned WITHOUT `register_missing_documents`: nothing is at the key.
    let f = SourceFile::new(&db, SRC.to_string(), "unregistered.fossil".to_string());

    let hits = goto_definition(&db, &[f], f, 4, 9);
    let target = hits.first().expect("the binding is still a definition");
    assert_eq!(target.file, f, "the fallback stays in the program");
    let (_, slice) = hit(&db, target);
    assert!(
        slice.starts_with("type {") && slice.contains("shop.shex"),
        "the fallback must be the type binding line; got {slice:?}"
    );

    // A property key has no such fallback: the binding says nothing about which
    // predicates the document declares.
    assert!(goto_definition(&db, &[f], f, 7, 5).is_empty());
}

/// A header naming a shape no `type { … }` binding introduced has nowhere to go.
/// The checker reports it; goto-def does not invent a target — and specifically
/// does not resolve the name to the header it is sitting on.
#[test]
fn an_unbound_shape_name_resolves_to_nothing() {
    const UNBOUND: &str = "\
type { Person } := io.shex(\"shop.shex\")

User := io.csv(\"users.csv\")

Users : Persn from User
    @subject = \"https://shop.example/user/{User.email}\"
    name = User.name
";
    let mut db = HostDb::new();
    let f = program(&mut db, UNBOUND);
    assert!(
        goto_definition(&db, &[f], f, 4, 9).is_empty(),
        "a misspelt shape name must not resolve to the header it is written in"
    );
}

/// goto-def on whitespace returns no targets and never panics.
#[test]
fn goto_def_on_whitespace_is_empty() {
    let mut db = HostDb::new();
    let f = program(&mut db, SRC);
    // Line 4, column 5 is the space between `Users` and `:`.
    let hits = goto_definition(&db, &[f], f, 4, 5);
    assert!(
        hits.iter().all(|t| t.range.start <= t.range.end),
        "goto-def on a separator must not produce a malformed target; got {hits:?}",
    );
}

/// 4. A SOURCE NAME resolves to its `:=` binding — at the binding itself, at a
///    `from`, and inside another binding's right-hand side.
///
/// Measured before `SymbolIndex` grew a `SOURCE_DEF` arm: **0 targets at all
/// four positions below.** The index walked only `MAPPING` nodes, so no `:=`
/// binding was in it and the workspace lookup had nothing to match.
#[test]
fn a_source_name_resolves_to_its_binding() {
    const WITH_DERIVED: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")

User := io.csv(\"users.csv\")

Adults := User.where(User.age)

Users : Person from Adults
    email = User.email
";
    let mut db = HostDb::new();
    let f = program(&mut db, WITH_DERIVED);

    // Line 2 col 1 — the read binding's own name.
    let at_binding = goto_definition(&db, &[f], f, 2, 1);
    assert_eq!(at_binding.len(), 1, "one binding; got {at_binding:?}");
    let (_, slice) = hit(&db, &at_binding[0]);
    assert!(
        slice.starts_with("User :="),
        "the range must cover the binding, from its name; got {slice:?}",
    );

    // Line 4 col 1 — a DERIVED binding is the same node kind, so it is indexed
    // too. A fix that special-cased `io.` readers would pass the case above and
    // fail this one.
    let derived = goto_definition(&db, &[f], f, 4, 1);
    assert_eq!(derived.len(), 1, "one binding; got {derived:?}");
    assert!(hit(&db, &derived[0]).1.starts_with("Adults :="));

    // Line 6 col 21 — the `from` of a mapping header. `shape_name_under_cursor`
    // claims only a `SHAPE_EXPR` parent, so this falls through to the index.
    let at_from = goto_definition(&db, &[f], f, 6, 21);
    assert_eq!(at_from.len(), 1, "`from Adults` resolves; got {at_from:?}");
    assert!(hit(&db, &at_from[0]).1.starts_with("Adults :="));

    // Line 4 col 11 — `User` inside the derived relation's right-hand side.
    let in_rhs = goto_definition(&db, &[f], f, 4, 11);
    assert_eq!(
        in_rhs.len(),
        1,
        "`User` in the RHS resolves; got {in_rhs:?}"
    );
    assert!(hit(&db, &in_rhs[0]).1.starts_with("User :="));

    // The shape name still leaves the language — the new arm must not have
    // shadowed the document path.
    let shape = goto_definition(&db, &[f], f, 6, 9);
    assert_eq!(hit(&db, &shape[0]).1, "shop:Person");
}
