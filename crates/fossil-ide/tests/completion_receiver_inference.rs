//! The receiver that needs INFERENCE, and the source that had no receiver at all.
//!
//! `5be2de6` narrowed the stdlib source to the cursor's receiver and wrote down
//! what it did not cover: `source_field_completions` still fired on every `.`
//! inside a mapping, and a head whose type has to be INFERRED — `Users.name.`,
//! a member of a COLUMN's type — resolved to nothing.
//!
//! Both are measured here, before and after, with a host that has done the one
//! thing the source-field path needs: registered an `InferredDescriptor` for the
//! URI the program's binding names. Without it every case below answers with
//! zero items and the file proves nothing, which is why
//! [`no_descriptor_no_columns`] pins that too.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::test_support::{DecodingHost, register_inferred};
use fossil_base::{Catalogue, Files, SourceFile, System};
use fossil_graph_schema::Primitive;
use lsp_types::CompletionItemKind;

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

/// A host that keeps a descriptor table AND decodes shape documents — the two
/// jobs `fossil-lsp`'s `LspSystem` does. `NativeSystem` alone keeps the table
/// but not the decoder; the bare `System` trait default keeps neither.
fn host() -> HostDb {
    HostDb {
        storage: salsa::Storage::default(),
        system: Arc::new(DecodingHost::default()),
        files: Files::default(),
        catalogue: Catalogue::default(),
    }
}

/// Intern `src` and register `users.csv` as a two-column row — `name` a String,
/// `age` an Integer. Two columns of DIFFERENT types, because the whole question
/// below is which type the member list comes from.
fn program(db: &mut HostDb, src: &str) -> SourceFile {
    register_inferred(
        db,
        "users.csv",
        &[("name", Primitive::String), ("age", Primitive::Integer)],
    );
    let f = SourceFile::new(&*db, src.to_string(), "receiver.fossil".to_string());
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

/// The cursor an editor puts one character past the `.` that triggered it: the
/// dot is the last character of the line, so the column is the line's length.
fn after_trailing_dot(src: &str, line: usize) -> u32 {
    let text = src.lines().nth(line).expect("the fixture has that line");
    assert!(text.ends_with('.'), "the fixture line must end in the dot");
    u32::try_from(text.len()).expect("a fixture line under u32::MAX")
}

/// `Users.name.` — the case the previous commit named and did not close.
///
/// **Before:** 0 FUNCTION items and 2 FIELD items — `name` and `age`, the
/// row's OWN columns, offered as if they were members of a String. The stdlib
/// source was correctly quiet (`name` is neither catalogued nor a binding) and
/// the source-field source was not asking who the receiver was at all, so the
/// only two items on offer were both wrong.
///
/// **After:** the 13 members a String has, and no column.
#[test]
fn a_string_column_offers_what_a_string_has() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.name.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));

    assert_eq!(
        labels(&items, CompletionItemKind::FUNCTION),
        vec![
            "concat",
            "contains",
            "ends_with",
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
/// Before: 2 FIELD items (`name`, `age`) — the same wrong two.
/// After: nothing at all, from either source.
#[test]
fn an_integer_column_offers_nothing_because_the_catalogue_has_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.age.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "an Integer has no catalogued members and is not a row; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// `str.` inside a mapping body. The stdlib source has been right about this
/// since `5be2de6`; the source-field source was adding the row's columns to it.
///
/// Before: 13 FUNCTION + 2 FIELD. After: 13 FUNCTION + 0 FIELD.
#[test]
fn a_catalogued_head_in_a_body_gets_no_columns() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = str.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
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
/// pins the stdlib half of this; the source-field half answered with the row's
/// columns.
///
/// Before: 0 FUNCTION + 2 FIELD. After: nothing.
#[test]
fn an_unknown_head_gets_no_columns_either() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = orders.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
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
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
    assert_eq!(
        labels(&items, CompletionItemKind::FIELD),
        vec!["name", "age"],
        "the row's columns, in descriptor order",
    );
    assert_eq!(
        labels(&items, CompletionItemKind::FUNCTION).len(),
        13,
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
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
    assert!(
        labels(&items, CompletionItemKind::FIELD).is_empty(),
        "no descriptor, no columns — the editor does not invent names; got {:?}",
        labels(&items, CompletionItemKind::FIELD),
    );
}

/// The partial member the very next keystroke produces — `users.name.tr|`.
/// The receiver is the same one; the cursor is inside the member being typed
/// rather than on the dot, and `scope_at_cursor` has handled both since
/// `5be2de6`. Before: 0 items, because the receiver was never resolved.
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
    let items = fossil_ide::completions(&db, &[f], f, 2, col);
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
/// Before: 2 FIELD items. After: nothing.
#[test]
fn a_column_the_row_does_not_have_offers_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.nope.
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "`nope` is not a column of the row; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// The leading `.` is a RETIRED form — `parser/expr.rs` refuses it by name
/// (`retired::LEADING_DOT`), because the row has a name and every reference is
/// qualified. It was nevertheless the trigger the source-field source used, and
/// two unit tests in `completion.rs` asserted its columns.
///
/// Before: 2 FIELD items. After: nothing — there is no receiver to the left of
/// a leading dot, so there is nothing to be a member of.
#[test]
fn the_retired_leading_dot_offers_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = .
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
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
/// Before: 2 FIELD items — the row's columns, wrong. After: nothing, which is
/// the honest answer until the arena holds sub-expressions.
#[test]
fn a_call_result_receiver_is_not_covered_and_says_nothing() {
    const SRC: &str = "\
users := io.csv(\"users.csv\")
User : Person from users
    name = users.name.trim().
";
    let mut db = host();
    let f = program(&mut db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, after_trailing_dot(SRC, 2));
    assert!(
        items.is_empty(),
        "a call result is not a receiver this resolves; got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}
