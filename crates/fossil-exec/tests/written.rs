//! **The program a model implies compiles and runs**: `fossil_ide::model::write` over files with
//! a key, a relation on the row, a link file whose headers are not names and an optional date,
//! checked by the whole native host with no diagnostic, run, and counted.

#[path = "support/native.rs"]
mod native;
mod support;

use fossil_graph_schema::Primitive;
use fossil_ide::model::{Model, ModelEdge, ModelField, ModelSource, ModelType, write};

const COUNTRIES: &str = "code,name\nPL,Poland\nDE,Germany\n";
const LAUREATES: &str = "id,name,born,country\n1,Marie Curie,1867-11-07,PL\n2,Albert Einstein,1879-03-14,DE\n3,Max Planck,,DE\n";
const INSTITUTIONS: &str = "id,city\nsorbonne,Paris\nkwi,Berlin\n";
const AFFILIATIONS: &str = "Laureate.id|Institution.id\n1|sorbonne\n2|kwi\n3|kwi\n";

fn source(name: &str, delimiter: Option<&str>) -> ModelSource {
    ModelSource {
        name: name.into(),
        format: "csv".into(),
        reference: format!("{name}.csv"),
        delimiter: delimiter.map(Into::into),
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
                fields: vec![field("name", Primitive::STRING, false)],
            },
            ModelType {
                name: "Laureates".into(),
                source: "laureates".into(),
                identity: vec!["id".into()],
                fields: vec![
                    field("name", Primitive::STRING, false),
                    field("born", Primitive::DATE, true),
                ],
            },
            ModelType {
                name: "Institutions".into(),
                source: "institutions".into(),
                identity: vec!["id".into()],
                fields: vec![field("city", Primitive::STRING, false)],
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

#[test]
fn the_program_a_model_implies_checks_clean_and_runs() {
    let program = write(&nobel(), "https://acme.example/");
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

    let ran = native::run_dir(dir.path(), "nobel.fossil", &[]).expect("the program runs");
    assert_eq!(ran.vertex_count("Countries"), 2);
    assert_eq!(ran.vertex_count("Laureates"), 3);
    assert_eq!(ran.vertex_count("Institutions"), 2);
    assert_eq!(ran.edge_count("Laureates_country_Countries"), 3);
    assert_eq!(ran.edge_count("Laureates_affiliation_Institutions"), 3);
}
