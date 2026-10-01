//! **The checked-in `problem.gen.ts` is what `problem.schema.json` says.**
//!
//! The third sibling of `catalogue_generated.rs` and `corpus_generated.rs`, and
//! it exists for their reason: the TypeScript union is printed from the schema,
//! so what is left to check is that somebody changed `Problem`, re-blessed the
//! schema and did not regenerate — or edited the generated file by hand. There
//! is no CI step for `cargo xtask problem --check`, for the reason `catalogue`
//! has none: `cargo test` runs this.
//!
//! # What this cannot prove
//!
//! - **That the schema is current.** `crates/fossil-graph-schema/tests/problem_schema.rs`
//!   holds the schema to the enum; this holds the TypeScript to the schema.
//!   Both must be green for the chain to be.
//! - **That `@fossil-lang/types` uses it well.** `FossilError` and its guard
//!   are held by that package's own vitest suite.

use xtask::catalogue::repo_root;
use xtask::problem;

/// The check `cargo xtask problem --check` runs, as a test.
#[test]
fn the_checked_in_file_matches_the_schema() {
    let root = repo_root();
    for (path, want) in problem::generated() {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let have = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{shown} is generated and must exist: {e}"));
        assert_eq!(
            have, want,
            "{shown} is stale — run `cargo xtask problem` and commit the result"
        );
    }
}

/// The guard has to be able to fail: a parse that found nothing would compare
/// two empty unions and pass.
#[test]
fn the_parse_actually_read_the_schema() {
    let codes = problem::read();
    assert!(
        codes.len() >= 30,
        "problem.schema.json parsed to {} codes",
        codes.len()
    );
    let over = codes
        .iter()
        .find(|c| c.code == "run/over-budget")
        .expect("run/over-budget is a code");
    let fields: Vec<(&str, &str, bool)> = over
        .fields
        .iter()
        .map(|(n, t, r)| (n.as_str(), t.as_str(), *r))
        .collect();
    assert_eq!(
        fields,
        [
            ("budget", "number", true),
            ("consumer", "string", true),
            ("requested", "number", true),
            ("reserved", "number", true),
        ]
    );
    assert!(
        codes.iter().all(|c| !c.title.is_empty()),
        "every code has a title"
    );
}

/// **A detail TypeScript renders reads as the one Rust renders.** For every code
/// whose `#[error]` the generator translates, data is built from the schema —
/// a string field `<name>-value`, a number 7, a list of two — and the rendering
/// the emitted TypeScript performs ([`problem::render`]) must equal `Problem`'s
/// own `Display` for that data. It is what keeps a detail raised in
/// `@fossil-lang/corpus` from reading differently from the same code raised in
/// Rust, which five of them did before `DETAILS` was generated.
///
/// It cannot prove that the TypeScript emitted for the segments renders as
/// `render` does: that is two lines of `emit_ts`, read once.
#[test]
fn every_translated_detail_reads_as_rust_renders_it() {
    let codes = problem::read();
    let mut checked = 0;
    for code in &codes {
        let Some(segments) = &code.detail else {
            continue;
        };
        let mut data = serde_json::Map::new();
        for (name, ty, required) in &code.fields {
            if !required {
                continue;
            }
            let value = match ty.as_str() {
                "string" => serde_json::json!(format!("{name}-value")),
                "number" => serde_json::json!(7),
                "boolean" => serde_json::json!(true),
                "string[]" => serde_json::json!([format!("{name}-a"), format!("{name}-b")]),
                "number[]" => serde_json::json!([1, 2]),
                other => panic!(
                    "`{}`'s `{name}` is a {other}, which no sample is built for",
                    code.code
                ),
            };
            data.insert(name.clone(), value);
        }
        let data = serde_json::Value::Object(data);
        let problem: fossil_graph_schema::Problem =
            serde_json::from_value(serde_json::json!({ "code": code.code, "data": data }))
                .unwrap_or_else(|e| panic!("`{}` reads back from its sample: {e}", code.code));
        assert_eq!(
            problem::render(segments, &data),
            problem.to_string(),
            "`{}`'s generated detail reads differently from Rust's",
            code.code
        );
        checked += 1;
    }
    assert!(
        checked > 50,
        "only {checked} details were translated — the parser reads nothing"
    );
}
