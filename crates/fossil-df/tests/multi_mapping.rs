//! Two mappings emitting the SAME vertex type from two sources must MERGE into
//! one table (UNION + dedup by subject), not register twice and clobber each
//! other (design §B4). Regression guard for the multi-mapping same-type path.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use datafusion::arrow::array::{Array, StringArray};
use datafusion::prelude::SessionContext;

mod support;

const PERSON_SHEX: &str = include_str!("fixtures/person-name.shex");

// `users` = person/1,2,3 (Alice,Bob,Carol); `extra` = person/3,4,5 (Carol,Dave,
// Eve). person/3 overlaps → single-valued dedup collapses it. Expect ONE Person
// table with 5 distinct subjects.
const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")

users := io.csv(\"tests/fixtures/users.csv\")
extra := io.csv(\"tests/fixtures/people_extra.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name

Person : Person from extra
    @subject = \"https://example.org/person/{extra.id}\"
    name = extra.name
";

#[tokio::test]
async fn same_type_from_two_sources_merges_into_one_table() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "merge.fossil", &[("person.shex", PERSON_SHEX)]);

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &fossil_df::OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    // Exactly ONE Person vertex table (not one per mapping).
    assert_eq!(
        graph.vertices.len(),
        1,
        "the two Person mappings merge into one table"
    );
    let person = &graph.vertices[0];
    assert_eq!(person.label, "Person");

    // 5 distinct subjects (person/3 deduped across the two sources), dense 0..4.
    let total: usize = person.batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        total, 5,
        "union of {{1,2,3}} and {{3,4,5}} deduped by subject = 5"
    );

    let subjects: Vec<String> = person
        .batches
        .iter()
        .flat_map(|b| {
            let col = b
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("subject is a string");
            (0..col.len())
                .map(|i| col.value(i).to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        subjects,
        [
            "https://example.org/person/1",
            "https://example.org/person/2",
            "https://example.org/person/3",
            "https://example.org/person/4",
            "https://example.org/person/5",
        ],
        "merged + sorted by subject",
    );

    // The single registered manifest/status reflects the merged count.
    let status = graph.run_status("mem://x");
    assert_eq!(status.vertices.len(), 1);
    assert_eq!(status.vertices[0].count, Some(5));
    let paths: Vec<String> = graph
        .manifests()
        .expect("manifests")
        .into_iter()
        .map(|m| m.rel_path)
        .collect();
    assert_eq!(
        paths,
        ["graph.graph.yml", "vertex/Person.vertex.yml"],
        "one vertex manifest, no duplicate Person entry",
    );
}
