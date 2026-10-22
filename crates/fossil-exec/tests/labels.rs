//! **What a run writes of `/docs/design/labels`**, and the counts its chunks
//! carry:
//! `docs/programs/labels` executed, and its graph read back — the information
//! files with `graphar-info`, the Parquet with `DuckDB`, never through the
//! writer's own types.
//!
//! The annotations a shape carries are not here: `GraphAr` has no word for them,
//! and they are the mapping's (`mapping.ttl`, which has its own tests).
//!
//! # What it does NOT prove
//!
//! - **That a reader reads them.** `@kanzo-tech/graphar-duckdb`'s views are held
//!   by its own tests.
//! - **That `distinct_count` is exact past what parquet-rs counts exactly.** It
//!   is the writer's count of one row group, and three prizes prove the
//!   field is there and counted, not its precision at scale.

use std::path::Path;

use graphar_info::{Cardinality, DataType};

#[path = "support/native.rs"]
mod native;
mod support;

fn labels() -> native::Written {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/programs/labels");
    native::run_dir(&dir, "labels.fossil", &[]).expect("labels.fossil runs")
}

/// A language-tagged value is the user-defined type `langString`, a set of
/// `(value, lang)` structs on the vertex type that owns it, whatever the field's
/// count; and the document that uses it declares it on its version line.
#[test]
fn a_lang_string_is_a_set_of_structs_on_its_vertex_type() {
    let ran = labels();
    for (ty, property) in [("Person", "name"), ("Prize", "motivation")] {
        let info = ran.vertex(ty);
        let p = info.property_groups[0]
            .properties
            .iter()
            .find(|p| p.name == property)
            .unwrap_or_else(|| panic!("{ty}.{property}"));
        assert_eq!(p.data_type, DataType::UserDefined("langString".to_string()));
        assert_eq!(p.cardinality, Cardinality::Set);
        assert_eq!(info.version.to_string(), "gar/v1 (langString)");
    }
    assert_eq!(
        ran.graph().version.to_string(),
        "gar/v1",
        "the graph document declares nothing it does not use"
    );
    assert_eq!(ran.vertex_count("Person"), 2);
    assert_eq!(ran.vertex_count("Prize"), 3);
    let root = ran.materialise();
    let names = native::rows(&format!(
        "SELECT n.value || '@' || n.lang AS v FROM (SELECT unnest(name) AS n FROM read_parquet({})) ORDER BY 1",
        native::at(root.path(), &ran.chunk("Person")),
    ));
    assert_eq!(
        names
            .into_iter()
            .map(|r| r[0].clone().expect("a name"))
            .collect::<Vec<_>>(),
        [
            "Albert Einstein@en",
            "Maria Skłodowska-Curie@pl",
            "Marie Curie@en"
        ]
    );
    let tags = native::rows(&format!(
        "SELECT n.lang FROM (SELECT unnest(motivation) AS n FROM read_parquet({})) ORDER BY 1",
        native::at(root.path(), &ran.chunk("Prize")),
    ));
    assert_eq!(
        tags.into_iter().map(|r| r[0].clone()).collect::<Vec<_>>(),
        vec![Some("en".to_string()); 3],
        "a constant tag is `rr:language`"
    );
}

/// Every property of every chunk is counted in the footer's own column
/// `Statistics` — nulls, bounds and `distinct_count` — Parquet's and not a key of
/// fossil's (`/docs/format/conventions/statistics`). A chunk is one row group,
/// so a row group's counts are the chunk's. A set is counted over its elements:
/// Parquet counts leaf values.
#[test]
fn the_footer_counts_what_was_written() {
    let ran = labels();
    let root = ran.materialise();
    let chunk = native::at(root.path(), &ran.chunk("Prize"));

    let distinct = native::rows(&format!(
        "SELECT path_in_schema, stats_distinct_count::VARCHAR FROM parquet_metadata({chunk}) \
         WHERE path_in_schema IN ('subject', 'category') OR path_in_schema LIKE 'motivation%value' \
         ORDER BY 1"
    ));
    let distinct: Vec<(String, String)> = distinct
        .into_iter()
        .map(|r| {
            (
                r[0].clone().expect("a column"),
                r[1].clone().expect("a distinct count"),
            )
        })
        .collect();
    assert_eq!(
        distinct
            .iter()
            .map(|(p, n)| (p.split(", ").last().unwrap_or(p).to_string(), n.as_str()))
            .collect::<Vec<_>>(),
        [
            ("category".to_string(), "2"),
            ("value".to_string(), "3"),
            ("subject".to_string(), "3"),
        ],
        "three prizes in two categories, three motivations: {distinct:?}"
    );

    // Nulls and bounds are the footer's own.
    let bounds = native::rows(&format!(
        "SELECT path_in_schema, stats_null_count::VARCHAR, stats_min_value, stats_max_value, \
                min_is_exact::VARCHAR, max_is_exact::VARCHAR \
         FROM parquet_metadata({chunk}) \
         WHERE path_in_schema IN ('category') OR path_in_schema LIKE 'motivation%value' ORDER BY 1"
    ));
    let cell = |row: &Vec<Option<String>>, i: usize| row[i].clone().expect("a statistic");
    let category = bounds
        .iter()
        .find(|r| r[0].as_deref() == Some("category"))
        .expect("category");
    assert_eq!(
        (
            cell(category, 1).as_str(),
            cell(category, 2).as_str(),
            cell(category, 3).as_str()
        ),
        ("0", "chemistry", "physics")
    );
    let motivation = bounds
        .iter()
        .find(|r| r[0].as_deref().is_some_and(|p| p.starts_with("motivation")))
        .expect("motivation");
    assert_eq!(
        cell(motivation, 2),
        "for his services to Theoretical Physics"
    );
    assert_eq!(
        cell(motivation, 5),
        "false",
        "the greatest motivation is longer than 64 bytes, and cut and marked inexact"
    );

    // No chunk carries a key of fossil's.
    let edge = native::at(root.path(), &ran.adjacency("Person_awarded_Prize"));
    let none = native::rows(&format!(
        "SELECT count(*)::VARCHAR FROM parquet_kv_metadata({edge}) WHERE key::VARCHAR LIKE 'fossil:%'"
    ));
    assert_eq!(none[0][0].as_deref(), Some("0"));
}

/// `write/invalid-language-tag` — a tag read from the data that is not BCP 47
/// refuses the run: no graph with an `rdf:langString` that has no valid tag.
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
    let Err(refused) = native::run_dir(dir.path(), "labels.fossil", &[]) else {
        panic!("the run is refused");
    };
    assert!(refused.contains("abcdefghi"), "{refused}");
    assert!(refused.contains("not a well-formed BCP 47"), "{refused}");
}
