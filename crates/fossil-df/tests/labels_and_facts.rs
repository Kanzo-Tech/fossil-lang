//! **What a run writes of `/docs/design/labels` and `/docs/design/facts`**:
//! `docs/programs/labels` executed, and its corpus read back — the manifest as
//! JSON, the Parquet with `DuckDB`, never through the writer's own types.
//!
//! # What it does NOT prove
//!
//! - **That a reader reads them.** `@fossil-lang/corpus`'s views are held by its
//!   own tests and its integration suite.
//! - **That the counts fit the budget on a large corpus.** Nothing here is
//!   measured; `/docs/design/facts` says what would be.

use std::path::Path;

use fossil_df::corpus::manifest::{Annotation, Inclusion};
use fossil_graph_schema::{RDF_LANG_STRING, RDFS_LABEL};

#[path = "support/native.rs"]
mod native;
mod support;

fn labels() -> native::Corpus {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/programs/labels");
    native::run_dir(&dir, "labels.fossil", &[]).expect("labels.fossil runs")
}

/// A language-tagged value is a property table of three columns, the tag
/// under `lang` with the role `language`, whatever the field's count; and the
/// value column says `rdf:langString`.
#[test]
fn a_lang_string_is_a_property_table_with_a_language_column() {
    let corpus = labels();
    let manifest = corpus.manifest();
    for (table, count) in [("Person_name", 3), ("Prize_motivation", 3)] {
        let t = manifest
            .property_tables
            .iter()
            .find(|t| t.name == table)
            .unwrap_or_else(|| panic!("{table}"));
        assert_eq!(t.record_count, count);
        let json = serde_json::to_value(&t.properties).unwrap();
        assert_eq!(json[2]["name"], "lang");
        assert_eq!(json[2]["role"], "language");
        assert_eq!(json[1]["datatype"], RDF_LANG_STRING);
    }
    let root = corpus.materialise();
    assert_eq!(
        native::column(root.path(), "property/Person_name.parquet", "name || '@' || lang"),
        ["Albert Einstein@en", "Maria Skłodowska-Curie@pl", "Marie Curie@en"]
    );
    assert_eq!(
        native::column(root.path(), "property/Prize_motivation.parquet", "lang"),
        ["en", "en", "en"],
        "a constant tag is `rr:language`"
    );
}

/// The annotations are carried onto the type, the field and the relation,
/// ShEx's Annotation as written.
#[test]
fn annotations_reach_the_manifest() {
    let manifest = labels().manifest();
    let person = manifest
        .vertex_tables
        .iter()
        .find(|v| v.name == "Person")
        .expect("Person");
    assert_eq!(
        person.annotations,
        [
            Annotation::literal(RDFS_LABEL, "Nobel laureate", Some("en")),
            Annotation::literal(RDFS_LABEL, "galardonado con el Nobel", Some("es")),
        ]
    );
    let born = person
        .properties
        .iter()
        .find(|p| p.name == "birthDate")
        .expect("birthDate");
    assert_eq!(born.annotations.len(), 2);
    let awarded = &manifest.edge_tables[0];
    assert_eq!(
        awarded.annotations,
        [Annotation::literal(RDFS_LABEL, "awarded", Some("en"))]
    );
    let text = String::from_utf8(
        labels().files.get("fossil.json").cloned().expect("fossil.json"),
    )
    .unwrap();
    assert!(
        text.contains("\"format\": \"fossil/1\""),
        "the format is changed in place"
    );
}

/// The writer counts every column of every table, names the candidate keys,
/// and finds the reference the program wrote as a string.
#[test]
fn the_writer_counts_what_it_wrote() {
    let manifest = labels().manifest();
    let prize = manifest
        .vertex_tables
        .iter()
        .find(|v| v.name == "Prize")
        .expect("Prize");
    assert_eq!(prize.candidate_keys, ["dense_id", "subject"]);
    let category = prize
        .properties
        .iter()
        .find(|p| p.name == "category")
        .and_then(|p| p.stats.as_ref())
        .expect("category is counted");
    assert_eq!(
        (category.null_count, category.distinct_count),
        (0, 2),
        "three prizes in two categories"
    );
    assert_eq!(category.min_value.as_deref(), Some("chemistry"));
    assert_eq!(category.max_value.as_deref(), Some("physics"));
    assert_eq!(category.top_values.len(), 1);
    assert_eq!(
        (category.top_values[0].value.as_str(), category.top_values[0].count),
        ("physics", 2)
    );
    let motivation = &manifest
        .property_tables
        .iter()
        .find(|t| t.name == "Prize_motivation")
        .expect("motivation")
        .properties[1];
    let stats = motivation.stats.as_ref().expect("counted");
    assert_eq!(
        stats.min_value.as_deref(),
        Some("for his services to Theoretical Physics")
    );
    assert_eq!(
        stats.max_value, None,
        "the greatest motivation is longer than 64 bytes, and left out rather than cut"
    );
    let edge = &manifest.edge_tables[0];
    assert!(edge.properties.iter().all(|p| p.stats.is_some()));
    // No program column here holds another table's key.
    assert_eq!(manifest.inclusions, Vec::<Inclusion>::new());
}

/// `write/invalid-language-tag` — a tag read from the data that is not BCP 47
/// refuses the run: no corpus with an `rdf:langString` that has no valid tag.
#[test]
fn a_per_row_tag_that_is_not_bcp_47_refuses_the_run() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/programs/labels");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("data")).expect("mkdir");
    for file in ["labels.fossil", "data/laureate.csv", "data/prize.csv"] {
        std::fs::copy(source.join(file), dir.path().join(file)).expect("copy");
    }
    std::fs::write(
        dir.path().join("data/laureate_name.csv"),
        "laureate,lang,name\n6,en,Marie Curie\n26,abcdefghi,Albert Einstein\n",
    )
    .expect("names");
    let refused = native::run_dir(dir.path(), "labels.fossil", &[])
        .err()
        .expect("the run is refused");
    assert!(refused.contains("abcdefghi"), "{refused}");
    assert!(refused.contains("not a well-formed BCP 47"), "{refused}");
}
