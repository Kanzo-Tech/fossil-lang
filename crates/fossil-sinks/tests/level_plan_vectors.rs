//! **The published level plan, executed** — the Rust half of the `level_plan`
//! section of `apps/corpus/guards/vectors.json`.
//!
//! There are two writers of a fossil corpus. [`VertexLevels::planned`] is one;
//! `apps/corpus/guards/fixture.mjs` is the other, plain Node with no cargo and
//! no npm, written from the published convention so that a corpus can be
//! produced by something that is not this repository. Which levels get written
//! is a decision both of them make, and a decision made twice is a decision that
//! drifts.
//!
//! **So the table is the deliverable and neither half wrote it.** This reads the
//! same bytes `guards/arithmetic.mjs` reads, at runtime rather than transcribed:
//! a border edited on one side turns the other side red, which a copied table
//! cannot do. It is the pattern `expected.json` already uses for the three
//! readers, applied to the two writers.
//!
//! The borders it carries are the ones an off-by-one moves: the floor exactly
//! and one row over it, a coarsest level holding exactly one tile, the floor
//! crossed by the SMALL term (600 rows at 8 to a tile), the conformance corpus
//! that is under it, and 2^53 + 1 where a `Number` search loses the tail.

use std::path::{Path, PathBuf};

use fossil_sinks::manifest::VertexLevels;
use serde_json::Value;

fn vectors_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/corpus/guards/vectors.json")
}

fn u(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|t| t.parse().ok()))
        .unwrap_or_else(|| panic!("not a row count: {value}"))
}

#[test]
fn the_published_level_plan_reproduces() {
    let path = vectors_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let table: Value = serde_json::from_str(&text).expect("vectors.json is JSON");
    let vectors = table["level_plan"]["vectors"]
        .as_array()
        .expect("level_plan.vectors is an array");

    // Non-vacuity, and it is not ceremony: this loop is over a table read from a
    // file, and a section renamed on the other side would leave it running zero
    // times with the test green.
    assert!(
        vectors.len() >= 5,
        "non-vacuity: {} vector(s), so the table was not read",
        vectors.len()
    );

    for vector in vectors {
        let count = u(&vector["count"]);
        let chunk_size = u(&vector["chunk_size"]);
        let want: Vec<u32> = vector["levels"]
            .as_array()
            .expect("levels is an array")
            .iter()
            .map(|l| u32::try_from(u(l)).expect("a level"))
            .collect();

        let plan = VertexLevels::planned(count, chunk_size);
        let got: Vec<u32> = plan.as_ref().map(|p| p.levels.clone()).unwrap_or_default();
        assert_eq!(
            got, want,
            "levels for {count} rows at {chunk_size} to a tile — {}",
            vector["why"]
        );

        // An empty plan is `None` and not `Some(vec![])`: a type under the floor
        // declares no `levels:` at all, and the manifest is silent about it.
        assert_eq!(
            plan.is_none(),
            want.is_empty(),
            "a plan of no levels must be absent rather than empty, at {count} rows"
        );

        // Every vector carries `rows`, empty where the plan is — the table is rendered by
        // `apps/docs/components/vector-table.tsx`, which takes its columns off the FIRST vector,
        // so a key present on some rows and not others is a column that disappears from the page.
        let rows = vector["rows"].as_array().expect("every vector declares rows");
        let got: Vec<u64> = plan
            .iter()
            .flat_map(|p| p.levels.iter().map(|&k| VertexLevels::rows_at(count, k)))
            .collect();
        let want: Vec<u64> = rows.iter().map(u).collect();
        assert_eq!(got, want, "rows per level at {count}");
    }
}
