//! Every file generated from a Rust type rather than a data file: the JSON
//! Schemas a reader outside Rust checks against, and the TypeScript the
//! `@fossil-lang/*` packages import instead of restating a shape.
//! `UPDATE_EXPECT=1 cargo test -p xtask --test wire` rewrites them.

use std::path::PathBuf;

use expect_test::expect_file;
use schemars::r#gen::SchemaSettings;
use serde_json::Value;
use xtask::catalogue::repo_root;
use xtask::{problem, ts};

fn at(path: &str) -> PathBuf {
    repo_root().join(path)
}

fn schema() -> Value {
    serde_json::from_str(&fossil_graph_schema::Problem::json_schema()).expect("the schema is JSON")
}

/// The error catalogue: `problem.schema.json`, and `problem.gen.ts` from it.
#[test]
fn the_error_catalogue() {
    expect_file![at("crates/fossil-graph-schema/problem.schema.json")]
        .assert_eq(&fossil_graph_schema::Problem::json_schema());
    let schema = schema();
    expect_file![at("packages/types/src/problem.gen.ts")].assert_eq(&problem::emit_ts(
        &problem::read(&schema),
        &schema["x-help"],
    ));
}

/// `fossil.json`: `fossil.schema.json`, and the reader's declarations of it.
#[test]
fn the_manifest() {
    use fossil_sinks::manifest::Manifest;
    let text = Manifest::json_schema();
    expect_file![at("crates/fossil-sinks/fossil.schema.json")].assert_eq(&text);
    let schema: Value = serde_json::from_str(&text).expect("the schema is JSON");
    expect_file![at("packages/corpus/src/manifest.gen.ts")].assert_eq(&format!(
        "{}{}",
        ts::header("`fossil_sinks::manifest::Manifest`"),
        ts::declarations(&schema)
    ));
}

/// What crosses `wasm-bindgen` and the `Host` contract, declared once in
/// `@fossil-lang/types`.
#[test]
fn the_wire() {
    // An absent optional is absent, never `null`: every one of these skips it.
    let mut generator = SchemaSettings::draft07()
        .with(|s| s.option_add_null_type = false)
        .into_generator();
    generator.subschema_for::<fossil_wasm::TokenRow>();
    generator.subschema_for::<fossil_wasm::CheckRow>();
    generator.subschema_for::<fossil_wasm::HoverRow>();
    generator.subschema_for::<fossil_wasm::CompletionRow>();
    generator.subschema_for::<fossil_wasm::DefinitionRow>();
    generator.subschema_for::<fossil_wasm::SemanticTokenRow>();
    generator.subschema_for::<fossil_wasm::MissingDocumentRow>();
    generator.subschema_for::<fossil_lineage::ProgramSource>();
    generator.subschema_for::<fossil_lineage::SourceRefInfo>();
    generator.subschema_for::<fossil_lineage::ProviderInfo>();
    generator.subschema_for::<fossil_descriptors_input::InferredDescriptor>();
    generator.subschema_for::<fossil_storage::StorageCredential>();
    generator.subschema_for::<fossil_storage::Access>();
    generator.subschema_for::<fossil_storage::Scope>();
    generator.subschema_for::<fossil_storage::GrantPlan>();
    generator.subschema_for::<fossil_storage::LocatorName>();
    generator.subschema_for::<fossil_df::RunReport>();
    let schema = serde_json::json!({ "definitions": generator.take_definitions() });
    // The figures both sides wait by.
    let constants = [
        ("HOST_MS", fossil_storage::HOST_MS),
        ("RENEW_BEFORE_MS", fossil_storage::RENEW_BEFORE_MS),
    ]
    .map(|(name, value)| {
        format!("\n/** `fossil_storage::{name}`. */\nexport const {name} = {value};\n")
    })
    .concat();
    expect_file![at("packages/types/src/wire.gen.ts")].assert_eq(&format!(
        "{}{}{constants}",
        ts::header("the types that cross `wasm-bindgen` and the `Host` contract"),
        ts::declarations(&schema)
    ));
}

/// The guard has to be able to fail: a parse that found nothing would compare
/// two empty unions and pass.
#[test]
fn the_parse_actually_read_the_schema() {
    let codes = problem::read(&schema());
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
    let codes = problem::read(&schema());
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
