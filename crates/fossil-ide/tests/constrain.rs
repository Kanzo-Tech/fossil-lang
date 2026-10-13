//! The mapping grammar is a sub-language of Fossil, held from both sides: what it generates parses,
//! and what the conformance programs write inside its scope, it generates.

use std::path::Path;
use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_ide::constrain::{
    Document, SourceVocabulary, TypeVocabulary, Vocabulary, mapping_grammar, prelude,
};
use fossil_syntax::parse;

fn source(
    name: &str,
    reference: &str,
    delimiter: Option<&str>,
    columns: &[&str],
) -> SourceVocabulary {
    SourceVocabulary {
        name: name.into(),
        format: "csv".into(),
        reference: reference.into(),
        delimiter: delimiter.map(Into::into),
        columns: columns.iter().map(|c| (*c).to_string()).collect(),
    }
}

fn shape(name: &str, properties: &[&str]) -> TypeVocabulary {
    TypeVocabulary {
        name: name.into(),
        properties: properties.iter().map(|p| (*p).to_string()).collect(),
    }
}

/// `docs/programs/relationship`'s vocabulary: its shapes and its sources' columns.
fn relationship() -> Vocabulary {
    Vocabulary {
        shapes: Document {
            format: "shex".into(),
            reference: "relationship.shex".into(),
        },
        types: vec![
            shape("Place", &["name"]),
            shape("Tag", &["name"]),
            shape(
                "Person",
                &["firstName", "isLocatedIn", "knows", "hasInterest"],
            ),
        ],
        sources: vec![
            source("PlaceRow", "data/place.csv", Some("|"), &["id", "name"]),
            source("TagRow", "data/tag.csv", Some("|"), &["id", "name"]),
            source(
                "PersonRow",
                "data/person.csv",
                Some("|"),
                &["id", "firstName", "place", "birthday"],
            ),
            source(
                "KnowsRow",
                "data/person_knows_person.csv",
                Some("|"),
                &["Person.id", "Person.id_1"],
            ),
            source(
                "InterestRow",
                "data/person_hasInterest_tag.csv",
                Some("|"),
                &["Person.id", "Tag.id"],
            ),
        ],
    }
}

fn hello() -> Vocabulary {
    Vocabulary {
        shapes: Document {
            format: "shex".into(),
            reference: "hello.shex".into(),
        },
        types: vec![shape("Person", &["name"])],
        sources: vec![source("User", "data/people.csv", None, &["id", "name"])],
    }
}

/// How many diagnostics the parser reports for `program`.
fn syntax_errors(program: &str) -> Vec<String> {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), "sampled.fossil".to_string());
    parse(&db, file)
        .diagnostics(&db)
        .iter()
        .map(|d| format!("{d:?}"))
        .collect()
}

/// The program's mappings in the grammar's one layout: comments, the prelude and alignment gone.
fn mappings(program: &str) -> String {
    let body: Vec<String> = program
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .skip_while(|l| !l.contains(" : "))
        .map(|l| {
            if let Some((key, value)) = l.split_once(" = ").filter(|_| l.starts_with("    ")) {
                format!("    {} = {}", key.trim(), value.trim())
            } else {
                l.trim_end().to_string()
            }
        })
        .collect();
    let mut text = body.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    format!("{}\n", text.trim_end())
}

fn program(name: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/programs");
    std::fs::read_to_string(root.join(format!("{name}/{name}.fossil"))).expect("the program")
}

#[test]
fn every_sentence_it_generates_parses() {
    for vocabulary in [hello(), relationship()] {
        let grammar = mapping_grammar(&vocabulary);
        for seed in 0..300 {
            let program = format!("{}\n{}", prelude(&vocabulary), grammar.sample(seed));
            let errors = syntax_errors(&program);
            assert!(
                errors.is_empty(),
                "seed {seed} generated a program the parser refuses:\n{program}\n{errors:#?}"
            );
        }
    }
}

#[test]
fn the_conformance_programs_in_its_scope_are_sentences_of_it() {
    for (name, vocabulary) in [("hello", hello()), ("relationship", relationship())] {
        let written = mappings(&program(name));
        assert!(
            mapping_grammar(&vocabulary).matches(&written),
            "{name}'s mappings are not a sentence:\n{written}"
        );
    }
}

#[test]
fn a_name_that_does_not_exist_is_not_a_sentence() {
    let grammar = mapping_grammar(&hello());
    let good = "People : Person from User\n    @subject = \"https://shop.example/person/{User.id}\"\n    name = User.name\n";
    assert!(grammar.matches(good));
    assert!(
        !grammar.matches(&good.replace("User.name", "User.nmae")),
        "a column the source does not have"
    );
    assert!(
        !grammar.matches(&good.replace("    name =", "    fullName =")),
        "a property the shape does not declare"
    );
    assert!(
        !grammar.matches(&good.replace(": Person", ": People")),
        "a type the shapes do not declare"
    );
}

#[test]
fn it_prints_as_ebnf_with_the_root_first_and_the_names_closed() {
    let ebnf = mapping_grammar(&hello()).ebnf();
    assert!(ebnf.starts_with("root ::= "), "{ebnf}");
    assert!(
        ebnf.contains("column ::= \"User.id\" | \"User.name\""),
        "{ebnf}"
    );
    assert!(
        ebnf.contains("\"parse.date\""),
        "the catalogue's parse functions: {ebnf}"
    );
}
