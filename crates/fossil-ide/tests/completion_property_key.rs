//! Where a property KEY can go, and what it is spelled.
//!
//! The shape-property source offers a mapping's output contract, and two things
//! decide whether what it offers can be written. Both are measured here against
//! `tests/fixtures/shop.shex` — `shop:Person` declaring `shop:email` and
//! `shop:name` — with the same host `fossil-wasm` installs.
//!
//! **The label.** A property key is never the predicate IRI: it is
//! [`fossil_graph_schema::local_name`] of it, the last segment, because
//! `PropertyLhs := IDENT` and `https://shop.example/voc#email` is not one.
//!
//! **The position.** A key goes at the start of a body line and nowhere else —
//! not the header, not the right of `=`, not after a member access. The stdlib
//! source is cut at the same position from the other side: no catalogued row is
//! an `IDENT`, so a key position offers the shape's keys and nothing else,
//! while every other position keeps the list it has.
//!
//! [`the_catalogue_is_not_offered_in_key_position`] holds the cut and
//! [`every_position_that_is_not_a_key_keeps_its_catalogue`] holds what it must
//! not reach, because a cut that took a position with it would be the same
//! defect pointing the other way.
//!
//! # What these tests do NOT prove
//!
//! - **That a name collision is handled.** The table comes from
//!   `ResolvedShape::short_names`, which drops the second of two predicates
//!   sharing a short name; no fixture here declares one, so that path is
//!   untested from this side.
//! - **Anything about a real editor.** Every position below is a `(line,
//!   character)` this file computes; no LSP client is driven.

use std::path::PathBuf;
use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile};
use lsp_types::CompletionItemKind;

fn host() -> FossilDb {
    FossilDb::new(Arc::new(NativeSystem::with_providers(
        fossil_hir::providers::PROVIDERS,
    )))
}

/// Intern `src` under a path NEXT TO the fixture document, so `io.shex("shop.shex")`
/// resolves the way it does for a program on disk, and register what it names.
fn program(db: &mut FossilDb, src: &str) -> SourceFile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join("property-key.fossil");
    let f = SourceFile::new(&*db, src.to_string(), path.to_string_lossy().into_owned());
    fossil_hir::documents::register_missing_documents(db, f, &|_, location| {
        std::fs::read_to_string(location).ok()
    });
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

fn fields(db: &FossilDb, f: SourceFile, line: u32, character: u32) -> Vec<String> {
    fossil_ide::completions(db, f, line, character)
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

/// A fresh, still-empty line inside the body — four spaces and nothing else,
/// which is what an editor leaves after `Enter` on an indented line.
///
/// The indent is written `\x20` because it is load-bearing and trailing
/// whitespace does not survive being looked at: four spaces are what an editor
/// leaves after `Enter`, and they are what puts the cursor at the body's
/// indentation. Without them `(4, 4)` is a column past the end of an empty
/// line, which `fossil_ide::offset` clamps to the line —
/// `crate::position`'s `token_at_position_past_the_end_of_a_line_does_not_panic`
/// is that guard.
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

/// **The position.** After a member access the receiver's members are the
/// answer, and a property key is not one of them.
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
            &fossil_ide::completions(&db, f, 3, 17),
            CompletionItemKind::FUNCTION
        )
        .len(),
        6,
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

/// **The position.** Outside every mapping there is no shape to draw from.
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

/// **A fresh body line is a key position.** Nothing is typed on it yet, so
/// there is no `PROPERTY_LHS` to stand on, and the parser attaches its indent
/// to the previous property's value; what makes it a key position is that the
/// indent is all that stands before the cursor, inside the mapping, at the
/// column its keys are written at. The next token typed there starts a
/// `Property`.
#[test]
fn a_blank_body_line_is_a_key_position() {
    let mut db = host();
    let f = program(&mut db, BLANK);
    assert_eq!(fields(&db, f, 4, 4), vec!["email", "name"]);
}

/// The column before an existing key's first character is the same position:
/// the start of a body line.
#[test]
fn the_column_before_a_key_is_a_key_position() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    assert_eq!(fields(&db, f, 3, 4), vec!["email", "name"]);
}

/// An indent that is not the body's is not the start of a key: a token there
/// would not be a statement of this body. Two columns into the four-space
/// indent, and a blank line indented past it, both answer no key.
#[test]
fn an_indent_that_is_not_the_bodys_is_not_a_key_position() {
    const DEEP: &str = "\
type { Person, Order } := io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")
Users : Person from User
    email = User.email
\x20\x20\x20\x20\x20\x20\x20\x20
";
    let mut db = host();
    let shop = program(&mut db, SHOP);
    assert!(fields(&db, shop, 3, 2).is_empty(), "mid-indent");
    let deep = program(&mut db, DEEP);
    assert!(fields(&db, deep, 4, 8).is_empty(), "deeper than the body");
}

/// The predicate IRI stays REACHABLE — as the item's detail, the way the
/// stdlib source keeps `str.trim` reachable behind the label `trim`. Moving it
/// out of the label must not delete it.
#[test]
fn the_detail_keeps_the_predicate_iri_and_leaks_no_type_state() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    let items = fossil_ide::completions(&db, f, 3, 9);
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

/// **The cut.** The catalogue is dotted and a key is a bare name, so in key
/// position the shape's predicates are the whole list — at the start of the
/// line, mid-name, and past the name's last keystroke.
#[test]
fn the_catalogue_is_not_offered_in_key_position() {
    let mut db = host();
    let f = program(&mut db, SHOP);
    for character in [4, 6, 9] {
        let items = fossil_ide::completions(&db, f, 3, character);
        let fns = labels(&items, CompletionItemKind::FUNCTION);
        assert!(
            fns.is_empty(),
            "no catalogued row parses as a `PropertyLhs`; got {fns:?} at column {character}",
        );
        assert_eq!(
            labels(&items, CompletionItemKind::FIELD),
            vec!["email", "name"],
            "the shape's predicates are the whole list at column {character}",
        );
        assert_eq!(items.len(), 2, "…and nothing else is offered");
    }
}

/// **The narrowing is a narrowing.** The catalogue is cut after a dot and in
/// key position, and a cut that took another position with it would be the
/// same class of mistake pointing the other way — so every position that is
/// NOT a key keeps the whole catalogue, or the receiver's members after a dot.
#[test]
fn every_position_that_is_not_a_key_keeps_its_catalogue() {
    let mut db = host();
    let shop = program(&mut db, SHOP);
    // Derived, not written down: what the test is about is that a non-key
    // position offers the catalogue WHOLE, a claim about a number matching the
    // catalogue's and not about a number.
    let whole = fossil_hir::stdlib::stdlib().iter().count();
    let verbs = fossil_hir::stdlib::stdlib()
        .members_of(fossil_hir::stdlib::Receiver::Relation)
        .count();
    assert!(
        whole > verbs && verbs > 0,
        "the catalogue yielded {whole} rows and {verbs} verbs; a count assertion \
         against an empty catalogue passes vacuously"
    );
    // (fixture, line, character, expected FUNCTION count, what it is)
    let cases: &[(SourceFile, u32, u32, usize, &str)] = &[
        (shop, 0, 0, whole, "the start of the file"),
        (shop, 1, 4, whole, "top level, outside any mapping"),
        (shop, 2, 3, whole, "the mapping header"),
        (shop, 3, 16, whole, "`User` on the right of `=`"),
        (
            shop,
            3,
            17,
            verbs,
            "`User.` — the relation verbs, untouched",
        ),
    ];
    for (f, line, character, expected, what) in cases {
        let items = fossil_ide::completions(&db, *f, *line, *character);
        assert_eq!(
            labels(&items, CompletionItemKind::FUNCTION).len(),
            *expected,
            "{what} at ({line}, {character})",
        );
    }
}
