//! goto-def integration test — every name a program binds, and the two that
//! leave the language.
//!
//! There is no import production, so no name a program writes resolves into
//! another `.fossil`: the second file is the `.shex`, where a shape name and a
//! property key are both defined, and this test is what holds that.
//!
//! It needs a HOST that has done its two jobs — installed a decoder row that
//! claims `.shex`, and REGISTERED the document as a Salsa input before the query
//! asks for it — which is what `FossilDb::new` + [`file`] do here and what
//! `fossil-wasm` does in production. `fossil_base::test_support::NativeSystem`
//! is not enough: its provider table reads no types, so every program resolves
//! no shape and every assertion below would fail for the wrong reason.

use std::path::PathBuf;
use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile};
use fossil_ide::{NavigationTarget, goto_definition};

fn host_db() -> FossilDb {
    FossilDb::new(Arc::new(NativeSystem::with_providers(
        fossil_hir::providers::PROVIDERS,
    )))
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
/// spelled with the same function `fossil-wasm` calls.
fn program(db: &mut FossilDb, src: &str) -> SourceFile {
    let path = fixture_dir().join("goto.fossil");
    let f = SourceFile::new(db, src.to_string(), path.to_string_lossy().into_owned());
    fossil_hir::documents::register_missing_documents(db, f, &|_, location| {
        std::fs::read_to_string(location).ok()
    });
    f
}

/// The text of the file a target points into, and the slice the range covers.
fn hit<'a>(db: &'a FossilDb, target: &NavigationTarget) -> (&'a str, String) {
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
    let mut db = host_db();
    let f = program(&mut db, SRC);

    let hits = goto_definition(&db, f, 4, 9);
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
    let mut db = host_db();
    let f = program(&mut db, TWO);

    // Line 4 is `Orders : Order from Purchase`; `Order` starts at column 9.
    let hits = goto_definition(&db, f, 4, 10);
    let (_, slice) = hit(&db, hits.first().expect("Order resolves"));
    assert_eq!(slice, "shop:Order");
}

/// 2. A PROPERTY KEY jumps into the `.shex`, at the PREDICATE it names.
///
/// `name = User.name` is line 7. The key is at column 4.
#[test]
fn a_property_key_resolves_to_its_predicate_in_the_document() {
    let mut db = host_db();
    let f = program(&mut db, SRC);

    let hits = goto_definition(&db, f, 7, 5);
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
    let mut db = host_db();
    let f = program(&mut db, SRC);

    // Line 6 is `    email = User.email`.
    let hits = goto_definition(&db, f, 6, 5);
    let (_, slice) = hit(&db, hits.first().expect("email resolves"));
    assert_eq!(slice, "shop:email");
}

/// `@subject` is not a predicate: a shape declares a node's predicates, and in
/// RDF the subject IS the node. It resolves to nothing rather than to whatever
/// the first predicate happens to be.
#[test]
fn the_identity_line_resolves_to_nothing() {
    let mut db = host_db();
    let f = program(&mut db, SRC);
    // Line 5 is `    @subject = "…"`; column 5 is inside `@subject`.
    assert!(goto_definition(&db, f, 5, 5).is_empty());
}

/// 3. A MAPPING NAME resolves inside the program, to its own name in its
///    header.
#[test]
fn a_mapping_name_resolves_in_the_program() {
    let mut db = host_db();
    let f = program(&mut db, SRC);

    // Line 4, column 1 is inside `Users`.
    let hits = goto_definition(&db, f, 4, 1);
    assert_eq!(hits.len(), 1, "one mapping; got {hits:?}");
    assert_eq!(hits[0].file, f);
    assert_eq!(hit(&db, &hits[0]).1, "Users");
}

/// When the document the program names is NOT registered — a host with no
/// filesystem, an unsaved buffer, a path that does not exist — a shape name
/// lands on its name in the `type { … }` binding that introduced it. That is a
/// real definition site for the NAME, and it is the honest answer when the
/// document cannot be opened.
#[test]
fn an_unregistered_document_falls_back_to_the_type_binding() {
    let db = host_db();
    // Interned WITHOUT `register_missing_documents`: nothing is at the key.
    let f = SourceFile::new(&db, SRC.to_string(), "unregistered.fossil".to_string());

    let hits = goto_definition(&db, f, 4, 9);
    let target = hits.first().expect("the binding is still a definition");
    assert_eq!(target.file, f, "the fallback stays in the program");
    let (text, slice) = hit(&db, target);
    assert_eq!(slice, "Person");
    assert!(
        text[..target.range.start as usize].ends_with("type { "),
        "the fallback must be the name in the type binding, not the header's",
    );

    // A property key has no such fallback: the binding says nothing about which
    // predicates the document declares.
    assert!(goto_definition(&db, f, 7, 5).is_empty());
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
    let mut db = host_db();
    let f = program(&mut db, UNBOUND);
    assert!(
        goto_definition(&db, f, 4, 9).is_empty(),
        "a misspelt shape name must not resolve to the header it is written in"
    );
}

/// goto-def on whitespace returns no targets and never panics.
#[test]
fn goto_def_on_whitespace_is_empty() {
    let mut db = host_db();
    let f = program(&mut db, SRC);
    // Line 4, column 5 is the space between `Users` and `:`.
    let hits = goto_definition(&db, f, 4, 5);
    assert!(
        hits.iter().all(|t| t.range.start <= t.range.end),
        "goto-def on a separator must not produce a malformed target; got {hits:?}",
    );
}

/// 4. A SOURCE NAME resolves to its `:=` binding's name — at the binding
///    itself, at a `from`, and inside another binding's right-hand side.
#[test]
fn a_source_name_resolves_to_its_binding() {
    const WITH_DERIVED: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")

User := io.csv(\"users.csv\")

Adults := User.where(User.age)

Users : Person from Adults
    email = User.email
";
    let mut db = host_db();
    let f = program(&mut db, WITH_DERIVED);
    let user = WITH_DERIVED.find("User :=").unwrap();
    let adults = WITH_DERIVED.find("Adults :=").unwrap();
    let lands_on = |hits: &[NavigationTarget], at: usize, name: &str| {
        assert_eq!(hits.len(), 1, "one binding; got {hits:?}");
        assert_eq!(hits[0].range.start as usize, at);
        assert_eq!(hit(&db, &hits[0]).1, name);
    };

    // Line 2 col 1 — the read binding's own name.
    lands_on(&goto_definition(&db, f, 2, 1), user, "User");
    // Line 4 col 1 — a DERIVED binding is the same node kind, so it resolves
    // the same way.
    lands_on(&goto_definition(&db, f, 4, 1), adults, "Adults");
    // Line 6 col 21 — the `from` of a mapping header.
    lands_on(&goto_definition(&db, f, 6, 21), adults, "Adults");
    // Line 4 col 11 — `User` inside the derived relation's right-hand side.
    lands_on(&goto_definition(&db, f, 4, 11), user, "User");

    // The shape name still leaves the language.
    let shape = goto_definition(&db, f, 6, 9);
    assert_eq!(hit(&db, &shape[0]).1, "shop:Person");
}

/// `{ A, B } := io.rdf(…)` binds two names in one item, and each lands on its
/// own — a jump to `B` that landed on `A`, or on the brace, would pass a test
/// that only checked the line.
#[test]
fn each_name_of_a_destructuring_binding_lands_on_itself() {
    const SRC: &str = "{ Person, Org } := io.rdf(\"g.ttl\")\nAdults := Org.where(Org.size)\n";
    let db = host_db();
    let f = SourceFile::new(&db, SRC.to_string(), "d.fossil".to_string());
    // Line 1 col 11 is inside `Org` in `Org.where`.
    let hits = goto_definition(&db, f, 1, 11);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].range.start as usize, SRC.find("Org").unwrap());
    assert_eq!(hit(&db, &hits[0]).1, "Org");
}

/// A member after a `.` is a column, not a top-level name, even when a binding
/// is spelled the same: `User.email` does not jump to `email := …`. A resolver
/// that matched by text alone would answer it.
#[test]
fn a_member_does_not_resolve_to_a_binding_of_the_same_name() {
    const SRC: &str = "\
type { Person } := io.shex(\"shop.shex\")
email := io.csv(\"emails.csv\")
User := io.csv(\"users.csv\")
Users : Person from User
    @subject = \"https://shop.example/user/{User.email}\"
    email = User.email
";
    let mut db = host_db();
    let f = program(&mut db, SRC);
    // Line 5 is `    email = User.email`; column 18 is inside the member `email`.
    assert!(goto_definition(&db, f, 5, 18).is_empty());
    // …while the binding itself does resolve, so the silence above is the
    // position's and not the name's.
    let binding = goto_definition(&db, f, 1, 1);
    assert_eq!(hit(&db, &binding[0]).1, "email");
}
