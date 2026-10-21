// `{laureates.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

//! The program a model implies, in its one layout. That it compiles and runs over real files is
//! `fossil-df`'s `tests/written.rs`; here is what it says.

use std::collections::HashMap;
use std::sync::Arc;

use expect_test::expect;
use fossil_base::sources::{InferredColumn, InferredDescriptor};
use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_graph_schema::Primitive;
use fossil_ide::model::{
    Inferred, Model, ModelEdge, ModelField, ModelSource, ModelType, infer, write,
};
use fossil_profile::{ColumnRef, ColumnStats, Inclusion, Profile, SourceProfile};
use fossil_syntax::parse;

const BASE: &str = "https://acme.example";

/// The parser's diagnostics for `program`.
fn syntax_errors(program: &str) -> Vec<String> {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), "written.fossil".to_string());
    parse(&db, file)
        .diagnostics(&db)
        .iter()
        .map(|d| format!("{d:?}"))
        .collect()
}

fn source(name: &str, reference: &str, option: Option<&str>) -> ModelSource {
    ModelSource {
        name: name.into(),
        format: "csv".into(),
        reference: reference.into(),
        option: option.map(Into::into),
    }
}

fn field(name: &str, column: &str, primitive: Primitive, optional: bool) -> ModelField {
    ModelField {
        name: name.into(),
        column: column.into(),
        primitive,
        optional,
    }
}

fn laureates(identity: &[&str]) -> ModelType {
    ModelType {
        name: "Laureates".into(),
        source: "laureates".into(),
        identity: identity.iter().map(|c| (*c).to_string()).collect(),
        fields: vec![
            field("name", "name", Primitive::String, false),
            field("born", "born", Primitive::Date, true),
        ],
    }
}

/// Laureates born in a country, affiliated with institutions through a link file whose headers
/// are not names.
fn nobel() -> Model {
    Model {
        sources: vec![
            source("countries", "countries.csv", None),
            source("laureates", "laureates.csv", None),
            source("institutions", "institutions.csv", None),
            source("affiliations", "affiliations.csv", Some("|")),
        ],
        types: vec![
            ModelType {
                name: "Countries".into(),
                source: "countries".into(),
                identity: vec!["code".into()],
                fields: vec![field("name", "name", Primitive::String, false)],
            },
            laureates(&["id"]),
            ModelType {
                name: "Institutions".into(),
                source: "institutions".into(),
                identity: vec!["id".into()],
                fields: vec![field("city", "city", Primitive::String, true)],
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
fn a_model_is_written_as_its_types_its_sources_and_a_mapping_per_type_and_link() {
    let program = write(&nobel(), BASE);
    expect![[r#"
        @iri("https://acme.example/vocab/Countries")
        type Countries
            @iri("https://acme.example/vocab/name") name: String

        @iri("https://acme.example/vocab/Laureates")
        type Laureates
            @iri("https://acme.example/vocab/name") name: String
            @iri("https://acme.example/vocab/born") born: Option<Date>
            @iri("https://acme.example/vocab/country") country: Ref<Countries>
            @iri("https://acme.example/vocab/affiliation") affiliation: Seq<Ref<Institutions>>

        @iri("https://acme.example/vocab/Institutions")
        type Institutions
            @iri("https://acme.example/vocab/city") city: Option<String>

        countries := io.csv("countries.csv")
        laureates := io.csv("laureates.csv")
        institutions := io.csv("institutions.csv")
        affiliations := io.csv("affiliations.csv", delimiter = "|")

        CountriesRows : Countries from countries
            @subject = "https://acme.example/id/countries/{countries.code}"
            name = countries.name

        LaureatesRows : Laureates from laureates
            @subject = "https://acme.example/id/laureates/{laureates.id}"
            name = laureates.name
            born = laureates.born
            country = Countries(laureates.country)

        LaureatesAffiliation : Laureates from laureates.join(affiliations, on = laureates.id == affiliations."Laureate.id")
            @subject = "https://acme.example/id/laureates/{laureates.id}"
            name = laureates.name
            country = Countries(laureates.country)
            affiliation = Institutions(affiliations."Institution.id")

        InstitutionsRows : Institutions from institutions
            @subject = "https://acme.example/id/institutions/{institutions.id}"
            city = institutions.city
    "#]]
    .assert_eq(&program);
    assert!(syntax_errors(&program).is_empty(), "{program}");
}

#[test]
fn the_same_model_writes_the_same_text() {
    assert_eq!(write(&nobel(), BASE), write(&nobel(), BASE));
    assert_eq!(
        write(&nobel(), BASE),
        write(&nobel(), "https://acme.example/"),
        "a trailing slash on the base is not a second slash"
    );
}

#[test]
fn a_subject_is_the_workspaces_prefix_the_types_words_and_the_identitys_holes() {
    let subject = |t: ModelType| {
        let model = Model {
            sources: vec![],
            types: vec![t],
            edges: vec![],
        };
        write(&model, BASE)
            .lines()
            .find_map(|l| l.strip_prefix("    @subject = "))
            .expect("a subject")
            .to_string()
    };
    assert_eq!(
        subject(laureates(&["laureate_id", "institution_id"])),
        "\"https://acme.example/id/laureates/{laureates.laureate_id}-{laureates.institution_id}\""
    );
    let named = |name: &str| {
        subject(ModelType {
            name: name.into(),
            ..laureates(&["id"])
        })
    };
    assert_eq!(
        named("LaureateAward"),
        "\"https://acme.example/id/laureate-award/{laureates.id}\""
    );
    assert_eq!(
        named("birth_date.2nd"),
        "\"https://acme.example/id/birth-date-2nd/{laureates.id}\""
    );
}

#[test]
fn a_relation_is_a_ref_an_optional_ref_or_a_sequence() {
    let ty = |many, optional| {
        let model = Model {
            sources: vec![],
            types: vec![laureates(&["id"])],
            edges: vec![ModelEdge {
                many,
                optional,
                ..nobel().edges[0].clone()
            }],
        };
        let program = write(&model, BASE);
        program
            .lines()
            .find_map(|l| l.split_once(" country: ").map(|(_, t)| t.to_string()))
            .expect("the relation is a field")
    };
    assert_eq!(ty(false, false), "Ref<Countries>");
    assert_eq!(ty(false, true), "Option<Ref<Countries>>");
    assert_eq!(ty(true, false), "Seq<Ref<Countries>>");
    assert_eq!(ty(true, true), "Seq<Ref<Countries>>");
}

/// Every primitive is written as the checker prints it, so a declared type reads back.
#[test]
fn every_primitive_is_a_type_the_program_declares() {
    let fields: Vec<ModelField> = Primitive::ALL
        .into_iter()
        .map(|primitive| field(&format!("f{primitive}"), "c", primitive, false))
        .collect();
    let model = Model {
        sources: vec![],
        types: vec![ModelType {
            fields,
            ..laureates(&["id"])
        }],
        edges: Vec::new(),
    };
    let text = write(&model, BASE);
    assert!(text.contains("fGYear: GYear"), "{text}");
    assert!(text.contains("fAnyUri: AnyUri"), "{text}");
    assert!(text.contains("fLangString: LangString"), "{text}");
    assert!(syntax_errors(&text).is_empty(), "{text}");
}

// ── infer ───────────────────────────────────────────────────────────────────

/// A profiled CSV: its columns, the ones with `nulls`, and its candidate keys.
fn profiled(binding: &str, columns: &[&str], nulls: &[&str], keys: &[&str]) -> SourceProfile {
    SourceProfile {
        binding: binding.into(),
        format: Some("csv".into()),
        option: None,
        descriptor: InferredDescriptor {
            key: format!("{binding}.csv").into(),
            columns: columns
                .iter()
                .map(|c| InferredColumn {
                    name: (*c).into(),
                    primitive: Primitive::String,
                })
                .collect(),
            etag: String::new(),
        },
        rows: Some(3),
        sampled: false,
        columns: columns
            .iter()
            .map(|c| ColumnStats {
                name: (*c).into(),
                null_count: u64::from(nulls.contains(c)),
                distinct_count: 3,
                min_value: None,
                max_value: None,
                top_values: Vec::new(),
            })
            .collect(),
        candidate_keys: keys.iter().map(|k| (*k).into()).collect(),
        sample: Vec::new(),
    }
}

fn inclusion(from: (&str, &str), to: (&str, &str)) -> Inclusion {
    Inclusion {
        from: ColumnRef {
            source: from.0.into(),
            column: from.1.into(),
        },
        to: ColumnRef {
            source: to.0.into(),
            column: to.1.into(),
        },
    }
}

fn inferred(profile: &Profile, choices: &[(&str, &str)]) -> Inferred {
    let choices: HashMap<String, String> = choices
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    infer(profile, &choices)
}

fn fields(t: &ModelType) -> Vec<&str> {
    t.fields.iter().map(|f| f.name.as_str()).collect()
}

#[test]
fn a_key_is_asked_only_where_no_inclusion_lands_on_one() {
    let people = || profiled("people", &["email", "id", "name"], &[], &["email", "id"]);
    let profile = Profile {
        sources: vec![people()],
        ..Profile::default()
    };
    let Inferred { model, ambiguities } = inferred(&profile, &[]);
    assert_eq!(ambiguities.len(), 1);
    assert_eq!(ambiguities[0].id, "key/people");
    assert_eq!(ambiguities[0].options, ["email", "id"]);
    assert_eq!(ambiguities[0].chosen, "email");
    assert_eq!(model.types[0].identity, ["email"]);
    assert_eq!(fields(&model.types[0]), ["id", "name"]);

    let model = inferred(&profile, &[("key/people", "id")]).model;
    assert_eq!(model.types[0].identity, ["id"]);
    assert_eq!(fields(&model.types[0]), ["email", "name"]);
    assert_eq!(
        inferred(&profile, &[("key/people", "name")]).model.types[0].identity,
        ["email"],
        "a choice that is not an option is not taken"
    );

    let fixed = Profile {
        sources: vec![
            people(),
            profiled("posts", &["slug", "author"], &[], &["slug"]),
        ],
        inclusions: vec![inclusion(("posts", "author"), ("people", "id"))],
        ..Profile::default()
    };
    let Inferred { model, ambiguities } = inferred(&fixed, &[]);
    assert!(ambiguities.is_empty(), "{ambiguities:?}");
    assert_eq!(model.types[0].identity, ["id"]);
}

#[test]
fn a_type_is_its_binding_in_pascal_case_and_a_relation_its_column_without_its_id() {
    let profile = Profile {
        sources: vec![
            profiled(
                "nobel_prizes",
                &["id", "country_id"],
                &["country_id"],
                &["id"],
            ),
            profiled("countries", &["code"], &[], &["code"]),
        ],
        inclusions: vec![inclusion(
            ("nobel_prizes", "country_id"),
            ("countries", "code"),
        )],
        ..Profile::default()
    };
    let model = inferred(&profile, &[]).model;
    assert_eq!(model.types[0].name, "NobelPrizes");
    assert_eq!(model.edges[0].name, "country");
    assert!(
        model.edges[0].optional,
        "a column with nulls is an optional relation"
    );
    assert!(fields(&model.types[0]).is_empty());

    let taken = Profile {
        sources: vec![
            profiled(
                "nobel_prizes",
                &["id", "country", "country_id"],
                &[],
                &["id"],
            ),
            profiled("countries", &["code"], &[], &["code"]),
        ],
        ..profile
    };
    let model = inferred(&taken, &[]).model;
    assert_eq!(fields(&model.types[0]), ["country"]);
    assert_eq!(
        model.edges[0].name, "country_id",
        "a relation whose short name a field holds keeps its column's"
    );
}

#[test]
fn two_columns_whose_words_make_one_name_are_asked_and_never_suffixed() {
    let profile = Profile {
        sources: vec![profiled(
            "orders",
            &["id", "order date", "order_date"],
            &[],
            &["id"],
        )],
        ..Profile::default()
    };
    let Inferred { model, ambiguities } = inferred(&profile, &[]);
    assert_eq!(ambiguities[0].id, "name/Orders/order_date");
    assert_eq!(ambiguities[0].options, ["order date", "order_date"]);
    let columns =
        |m: &Model| -> Vec<String> { m.types[0].fields.iter().map(|f| f.column.clone()).collect() };
    assert_eq!(columns(&model), ["order date"]);
    let model = inferred(&profile, &[("name/Orders/order_date", "order_date")]).model;
    assert_eq!(columns(&model), ["order_date"]);
}

#[test]
fn a_source_nothing_identifies_is_left_out_and_not_bound() {
    let profile = Profile {
        sources: vec![
            profiled("people", &["id"], &[], &["id"]),
            profiled("log", &["at", "who"], &[], &[]),
        ],
        inclusions: vec![inclusion(("log", "who"), ("people", "id"))],
        ..Profile::default()
    };
    let Inferred { model, ambiguities } = inferred(&profile, &[]);
    assert!(ambiguities.is_empty());
    let bound: Vec<&str> = model.sources.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(bound, ["people"]);
    assert!(model.edges.is_empty());
}
