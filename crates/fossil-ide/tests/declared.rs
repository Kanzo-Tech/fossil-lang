//! The editor's questions over a type declared in the program: the key position offers its
//! fields, a key's hover shows the field's type, and the type's name and a key go to the
//! declaration — the same answers an imported type gets from its document.

// `"…/{Row.id}"` is literal Fossil source, not a Rust format-string argument.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::sources::{InferredColumn, InferredDescriptor};
use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile};
use fossil_graph_schema::Primitive;
use lsp_types::CompletionItemKind;

/// Line 7 is `    name = Row.name`.
const PROGRAM: &str = "\
@iri(\"https://schema.org/Person\")
type Person
    @iri(\"https://schema.org/name\")          name: String
    @iri(\"https://shop.example/vocab/name\")  handle: Option<String>
Row := io.csv(\"people.csv\")
People : Person from Row
    @subject = \"https://shop.example/p/{Row.name}\"
    name = Row.name
";

fn program() -> (FossilDb, SourceFile) {
    let mut db = FossilDb::new(Arc::new(NativeSystem::with_providers(
        fossil_hir::providers::PROVIDERS,
    )));
    fossil_base::describe_columns(
        &mut db,
        InferredDescriptor {
            key: "people.csv".into(),
            columns: vec![InferredColumn {
                name: "name".into(),
                primitive: Primitive::STRING,
            }],
            etag: String::new(),
        },
    );
    let file = SourceFile::new(&db, PROGRAM.to_string(), "/p/people.fossil".to_string());
    (db, file)
}

fn text_at(file: SourceFile, db: &FossilDb, range: std::ops::Range<u32>) -> String {
    file.text(db)[range.start as usize..range.end as usize].to_string()
}

#[test]
fn the_key_position_offers_a_declared_types_fields() {
    let (db, file) = program();
    let fields: Vec<String> = fossil_ide::completions(&db, file, 7, 6)
        .into_iter()
        .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
        .map(|i| i.label)
        .collect();
    assert_eq!(fields, vec!["name", "handle"]);
}

#[test]
fn a_keys_hover_shows_the_declared_fields_type() {
    let (db, file) = program();
    let hover = fossil_ide::hover(&db, file, 7, 15).expect("a hover on Row.name");
    assert!(hover.markdown.contains("String"), "{}", hover.markdown);
}

#[test]
fn the_type_name_goes_to_its_declaration() {
    let (db, file) = program();
    let targets = fossil_ide::goto_definition(&db, file, 5, 10);
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].file, file);
    let declaration = PROGRAM.find("type Person").unwrap() + "type ".len();
    assert_eq!(targets[0].range.start as usize, declaration);
    assert_eq!(
        text_at(targets[0].file, &db, targets[0].range.clone()),
        "Person"
    );
}

#[test]
fn a_key_goes_to_its_field() {
    let (db, file) = program();
    let targets = fossil_ide::goto_definition(&db, file, 7, 5);
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].file, file);
    let field = PROGRAM.find("name: String").unwrap();
    assert_eq!(targets[0].range.start as usize, field);
    assert_eq!(
        text_at(targets[0].file, &db, targets[0].range.clone()),
        "name"
    );
}

#[test]
fn a_mapping_without_its_subject_is_told_once() {
    let (db, _) = program();
    let text = PROGRAM.replace("    @subject = \"https://shop.example/p/{Row.name}\"\n", "");
    let file = SourceFile::new(&db, text, "/p/no-subject.fossil".to_string());
    let missing: Vec<_> = fossil_ide::diagnostics(&db, file)
        .into_iter()
        .filter(|d| d.problem.code() == "identity/missing-subject")
        .collect();
    assert_eq!(missing.len(), 1, "{missing:#?}");
}
