//! **The program a model implies compiles and runs**: `fossil_ide::model::write` over files with
//! a key, a relation on the row, a link file whose headers are not names and an optional date,
//! checked by the whole native host with no diagnostic, run, and counted — the model written by
//! hand, and the model `fossil_ide::model::infer` reads off the files' profile, at each answer
//! to the question it asks.

#[path = "support/native.rs"]
mod native;
mod support;

use std::collections::HashMap;
use std::path::Path;

use fossil_graph_schema::Primitive;
use fossil_ide::model::{
    Inferred, Model, ModelEdge, ModelField, ModelSource, ModelType, infer, write,
};
use fossil_profile::{Options, Source};

const COUNTRIES: &str = "code,name\nPL,Poland\nDE,Germany\n";
const LAUREATES: &str = "id,name,born,country\n1,Marie Curie,1867-11-07,PL\n2,Albert Einstein,1879-03-14,DE\n3,Max Planck,,DE\n";
const INSTITUTIONS: &str = "id,city\nsorbonne,Paris\nkwi,Berlin\n";
const AFFILIATIONS: &str = "Laureate.id|Institution.id\n1|sorbonne\n2|kwi\n2|sorbonne\n3|kwi\n";

fn source(name: &str, option: Option<&str>) -> ModelSource {
    ModelSource {
        name: name.into(),
        format: "csv".into(),
        reference: format!("{name}.csv"),
        option: option.map(Into::into),
    }
}

fn field(name: &str, primitive: Primitive, optional: bool) -> ModelField {
    ModelField {
        name: name.into(),
        column: name.into(),
        primitive,
        optional,
    }
}

fn nobel() -> Model {
    Model {
        sources: vec![
            source("countries", None),
            source("laureates", None),
            source("institutions", None),
            source("affiliations", Some("|")),
        ],
        types: vec![
            ModelType {
                name: "Countries".into(),
                source: "countries".into(),
                identity: vec!["code".into()],
                fields: vec![field("name", Primitive::String, false)],
            },
            ModelType {
                name: "Laureates".into(),
                source: "laureates".into(),
                identity: vec!["id".into()],
                fields: vec![
                    field("name", Primitive::String, false),
                    field("born", Primitive::Date, true),
                ],
            },
            ModelType {
                name: "Institutions".into(),
                source: "institutions".into(),
                identity: vec!["id".into()],
                fields: vec![field("city", Primitive::String, false)],
            },
        ],
        edges: vec![
            ModelEdge {
                name: "country".into(),
                from: "Laureates".into(),
                to: "Countries".into(),
                on: "laureates".into(),
                column: "country".into(),
                join: None,
                many: false,
                optional: false,
            },
            ModelEdge {
                name: "affiliation".into(),
                from: "Laureates".into(),
                to: "Institutions".into(),
                on: "affiliations".into(),
                column: "Institution.id".into(),
                join: Some("Laureate.id".into()),
                many: true,
                optional: false,
            },
        ],
    }
}

/// The run of the program `model` implies over the Nobel files, after it checked clean.
fn checked_and_run(model: &Model) -> native::Written {
    let program = write(model, "https://acme.example/");
    let dir = native::write_dir(&[
        ("nobel.fossil", &program),
        ("countries.csv", COUNTRIES),
        ("laureates.csv", LAUREATES),
        ("institutions.csv", INSTITUTIONS),
        ("affiliations.csv", AFFILIATIONS),
    ]);
    let messages: Vec<String> = native::check(&dir.path().join("nobel.fossil"))
        .diagnostics
        .iter()
        .map(fossil_base::Diagnostic::message)
        .collect();
    assert!(messages.is_empty(), "{program}\n{messages:#?}");

    native::run_dir(dir.path(), "nobel.fossil", &[]).expect("the program runs")
}

#[test]
fn the_program_a_model_implies_checks_clean_and_runs() {
    let ran = checked_and_run(&nobel());
    assert_eq!(ran.vertex_count("Countries"), 2);
    assert_eq!(ran.vertex_count("Laureates"), 3);
    assert_eq!(ran.vertex_count("Institutions"), 2);
    assert_eq!(ran.edge_count("Laureates_country_Countries"), 3);
    assert_eq!(ran.edge_count("Laureates_affiliation_Institutions"), 4);
}

/// The model the Nobel files' profile implies, at `choices`.
fn inferred(choices: &[(&str, &str)]) -> Inferred {
    let dir = native::write_dir(&[
        ("countries.csv", COUNTRIES),
        ("laureates.csv", LAUREATES),
        ("institutions.csv", INSTITUTIONS),
        ("affiliations.csv", AFFILIATIONS),
    ]);
    let source = |name: &str, option: Option<&str>| Source {
        binding: name.into(),
        key: format!("{name}.csv").into(),
        location: dir.path().join(format!("{name}.csv")).display().to_string(),
        format: "csv".into(),
        option: option.map(Into::into),
        etag: String::new(),
    };
    let profile = native::profile::profile(
        vec![
            source("countries", None),
            source("laureates", None),
            source("institutions", None),
            source("affiliations", Some("|")),
        ],
        Options::WHOLE,
    );
    assert!(profile.failed.is_empty(), "{:?}", profile.failed);
    let choices: HashMap<String, String> = choices
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    infer(&profile, &choices)
}

#[test]
fn the_program_a_profile_implies_checks_clean_and_runs_a_link_file_as_a_relation() {
    let Inferred { model, ambiguities } = inferred(&[]);
    let asked: Vec<(&str, &str)> = ambiguities
        .iter()
        .map(|a| (a.id.as_str(), a.chosen.as_str()))
        .collect();
    assert_eq!(asked, [("count/affiliations", "link")], "{model:#?}");

    let ran = checked_and_run(&model);
    assert_eq!(ran.vertex_count("Countries"), 2);
    assert_eq!(ran.vertex_count("Laureates"), 3);
    assert_eq!(ran.vertex_count("Institutions"), 2);
    assert_eq!(ran.edge_count("Laureates_country_Countries"), 3);
    assert_eq!(ran.edge_count("Laureates_Institution_Institutions"), 4);
}

#[test]
fn the_program_a_profile_implies_checks_clean_and_runs_a_link_file_as_a_type() {
    let Inferred { model, .. } = inferred(&[("count/affiliations", "thing")]);
    let ran = checked_and_run(&model);
    assert_eq!(ran.vertex_count("Affiliations"), 4);
    assert_eq!(ran.edge_count("Affiliations_Laureate_Laureates"), 4);
    assert_eq!(ran.edge_count("Affiliations_Institution_Institutions"), 4);
}

#[test]
fn a_model_is_inferred_from_the_files_own_words() {
    let model = inferred(&[]).model;
    let types: Vec<(&str, &[String])> = model
        .types
        .iter()
        .map(|t| (t.name.as_str(), t.identity.as_slice()))
        .collect();
    assert_eq!(
        types,
        [
            ("Countries", &["code".to_string()][..]),
            ("Laureates", &["id".to_string()][..]),
            ("Institutions", &["id".to_string()][..]),
        ]
    );
    let laureates = &model.types[1];
    let fields: Vec<(&str, Primitive, bool)> = laureates
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.primitive, f.optional))
        .collect();
    assert_eq!(
        fields,
        [
            ("name", Primitive::String, false),
            ("born", Primitive::Date, true),
        ],
        "the column a relation leaves from is not a field"
    );
    assert_eq!(model.sources[3].option.as_deref(), Some("|"));
    assert!(Path::new(&model.sources[0].reference).is_relative());
}
