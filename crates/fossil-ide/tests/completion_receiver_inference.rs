//! The receiver that needs INFERENCE, and the columns that belong to one
//! receiver only.
//!
//! A head whose type has to be INFERRED — `Users.name.`, a member of a
//! COLUMN's type — offers that type's members, and the row's columns are
//! offered after the row's own name and after nothing else.
//!
//! Both are measured here with a host that has done the one thing the
//! source-field path needs: registered an `InferredDescriptor` for the URI the
//! program's binding names. Without it every case below answers with zero items
//! and the file proves nothing, which is why [`no_descriptor_no_columns`] pins
//! that too.

use std::sync::Arc;

use fossil_base::test_support::{NativeSystem, register_inferred};
use fossil_base::{FossilDb, SourceFile};
use fossil_graph_schema::Primitive;
use lsp_types::CompletionItemKind;

/// A host that describes its sources AND decodes shape documents — the two
/// jobs the browser host's `System` does. `NativeSystem` alone keeps the table
/// but not the decoder; the bare `System` trait default keeps neither.
fn host() -> FossilDb {
    FossilDb::new(Arc::new(NativeSystem::decoding()))
}

/// Intern `src` and register `users.csv` as a two-column row — `name` a String,
/// `age` an Integer. Two columns of DIFFERENT types, because the whole question
/// below is which type the member list comes from.
fn program(db: &mut FossilDb, src: &str) -> SourceFile {
    register_inferred(
        db,
        "users.csv",
        &[("name", Primitive::String), ("age", Primitive::Integer)],
    );
    let f = SourceFile::new(&*db, src.to_string(), "receiver.fossil".to_string());
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

/// The cursor an editor puts one character past the `.` that triggered it: the
/// dot is the last character of the line, so the column is the line's length.
fn after_trailing_dot(src: &str, line: usize) -> u32 {
    let text = src.lines().nth(line).expect("the fixture has that line");
    assert!(text.ends_with('.'), "the fixture line must end in the dot");
    u32::try_from(text.len()).expect("a fixture line under u32::MAX")
}

/// `Users.name.` — a member of a String column: the members a String has, and
/// no column. The row's own columns, `name` and `age`, are not members of a
/// String, and a source that did not ask who the receiver is would offer them
/// here.
#[test]
fn a_string_column_offers_what_a_string_has() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.name.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));

    assert_eq!(
        labels(&items, CompletionItemKind::FUNCTION),
        vec![
            "contains",
            "ends_with",
            "lang",
            "length",
            "lower",
            "replace",
            "slice",
            "slug",
            "split",
            "starts_with",
            "strip_html",
            "trim",
            "upper",
        ],
        "`users.name` is a String: its members are `str.*`, sorted, bare",
    );
    assert!(
        labels(&items, CompletionItemKind::FIELD).is_empty(),
        "the row's columns are not members of one of its columns; got {:?}",
        labels(&items, CompletionItemKind::FIELD),
    );
    // The dotted spelling stays reachable as the detail, the way it does for a
    // catalogued head.
    let trim = items.iter().find(|i| i.label == "trim").expect("trim");
    assert!(
        trim.detail
            .as_deref()
            .unwrap_or("")
            .starts_with("str.trim("),
        "the detail must name the row's full dotted signature; got {:?}",
        trim.detail,
    );
}

/// `Users.age.` — an Integer column. The catalogue has `str.*` and `seq.*` and
/// no row whose receiver is any other scalar, so the honest list is EMPTY.
///
/// Nothing at all, from either source.
#[test]
fn an_integer_column_offers_nothing_because_the_catalogue_has_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.age.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "an Integer has no catalogued members and is not a row; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// `str.` inside a mapping body: what a string has, and none of the row's
/// columns — `str` is not the row.
#[test]
fn a_catalogued_head_in_a_body_gets_no_columns() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = str.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert!(
        labels(&items, CompletionItemKind::FUNCTION).contains(&"trim"),
        "`str.` still offers what a string has; got {:?}",
        labels(&items, CompletionItemKind::FUNCTION),
    );
    assert!(
        labels(&items, CompletionItemKind::FIELD).is_empty(),
        "`str` is not the row: it has no columns; got {:?}",
        labels(&items, CompletionItemKind::FIELD),
    );
}

/// A head that names nothing — `orders.`. `an_unknown_head_offers_no_catalogue_row`
/// pins the stdlib half of this; this is the source-field half: nothing.
#[test]
fn an_unknown_head_gets_no_columns_either() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = orders.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "`orders` names nothing, in either source; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// The row's own name still offers the row's columns — the case that must NOT
/// go quiet. Unchanged by this commit and pinned so a later narrowing cannot
/// take it away: 2 FIELD items before and after.
///
/// It also offers the 13 relation verbs, and that is the receiver question this
/// commit does NOT answer: a binding is a relation in a `:=` right-hand side
/// and a ROW in a mapping body, the CST tells the two apart, and completion
/// does not ask. Pinned as-is rather than changed, because reversing it is a
/// second measurement and a second commit.
#[test]
fn the_row_binding_still_offers_its_columns() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert_eq!(
        labels(&items, CompletionItemKind::FIELD),
        vec!["name", "age"],
        "the row's columns, in descriptor order",
    );
    assert_eq!(
        labels(&items, CompletionItemKind::FUNCTION).len(),
        6,
        "the relation verbs are still offered here — see this test's docs",
    );
}

/// With no descriptor registered for the URI the binding names, every case
/// above is empty for a reason that has nothing to do with the receiver. This
/// is what keeps the others from passing vacuously.
#[test]
fn no_descriptor_no_columns() {
    const SRC: &str = "\
orders := io.csv(\"orders.csv\")
Order : Thing from orders
    name = orders.
";
    let mut db = host();
    // `program` registers `users.csv`, which this program does not name.
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert!(
        labels(&items, CompletionItemKind::FIELD).is_empty(),
        "no descriptor, no columns — the editor does not invent names; got {:?}",
        labels(&items, CompletionItemKind::FIELD),
    );
}

/// The partial member the very next keystroke produces — `users.name.tr|`.
/// The receiver is the same one; the cursor is inside the member being typed
/// rather than on the dot, and `scope_at_cursor` reads the head the same way
/// from both.
#[test]
fn a_partial_member_resolves_the_same_receiver() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.name.tr
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let col = u32::try_from(SRC.lines().nth(2).unwrap().len()).unwrap();
    let items = fossil_ide::completions(&db, f, 2, col);
    assert!(
        labels(&items, CompletionItemKind::FUNCTION).contains(&"trim"),
        "`users.name.tr|` is still a String receiver; got {:?}",
        labels(&items, CompletionItemKind::FUNCTION),
    );
}

/// A column the descriptor does not declare — `users.nope.`. The head is
/// qualified by the row, so the walk gets one step further than for `orders.`
/// and then finds no field. Silence is the answer; the row's columns are not.
///
/// Nothing.
#[test]
fn a_column_the_row_does_not_have_offers_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.nope.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "`nope` is not a column of the row; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// A leading `.` has no receiver, so there is nothing to be a member of.
#[test]
fn a_leading_dot_offers_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = .
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "a leading `.` starts nothing; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// A receiver that is a CALL's result — `users.name.trim().`. Not covered, and
/// pinned so the boundary is a measurement rather than a hope: the head to the
/// left of the dot is a `)`, and typing a sub-expression needs an `ExprId` the
/// body arena does not mint (`hover.rs` records the same limit: one entry per
/// property VALUE, so a sub-expression has no id).
///
/// Nothing, which is the honest answer until the arena holds sub-expressions.
#[test]
fn a_call_result_receiver_is_not_covered_and_says_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.name.trim().
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "a call result is not a receiver this resolves; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// A column whose header is not an identifier — LDBC's `Person.id` — is
/// labelled by its name and INSERTED in its one spelling, `"Person.id"`, which
/// is what the parser takes after the dot. An identifier is inserted as itself.
#[test]
fn a_column_an_identifier_cannot_spell_is_inserted_quoted() {
    const SRC: &str = "\
knows := io.csv(\"knows.csv\")
User : Person from knows
    name = knows.
";
    let mut db = host();
    register_inferred(
        &mut db,
        "knows.csv",
        &[
            ("Person.id", Primitive::String),
            ("Person.id_1", Primitive::String),
            ("since", Primitive::Integer),
        ],
    );
    let f = SourceFile::new(&db, SRC.to_string(), "receiver.fossil".to_string());
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    let fields: Vec<(&str, Option<&str>)> = items
        .iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
        .map(|i| (i.label.as_str(), i.insert_text.as_deref()))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("Person.id", Some("\"Person.id\"")),
            ("Person.id_1", Some("\"Person.id_1\"")),
            ("since", None),
        ],
    );
}

/// `knows."first name".` — a quoted column is a head like a bare one: the
/// receiver is that column's type, so a String column offers `str.*`.
#[test]
fn a_quoted_column_is_a_receiver_like_a_bare_one() {
    const SRC: &str = "\
knows := io.csv(\"knows.csv\")
User : Person from knows
    name = knows.\"first name\".
";
    let mut db = host();
    register_inferred(&mut db, "knows.csv", &[("first name", Primitive::String)]);
    let f = SourceFile::new(&db, SRC.to_string(), "receiver.fossil".to_string());
    let items = fossil_ide::completions(&db, f, 2, after_trailing_dot(SRC, 2));
    let functions = labels(&items, CompletionItemKind::FUNCTION);
    assert!(functions.contains(&"trim"), "{functions:?}");
    assert!(labels(&items, CompletionItemKind::FIELD).is_empty());
}
