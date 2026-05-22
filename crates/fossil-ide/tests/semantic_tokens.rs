//! Snapshot test for `fossil_ide::semantic_tokens` (LSP-01 / SC#4).
//!
//! The raw LSP output is a flat `Vec<u32>` of delta-encoded 5-tuples — opaque to
//! review. We decode it back to absolute `(line, col, len, type)` rows and render
//! each with its human-readable legend type name, so the snapshot diffs as a
//! readable table when the classifier or fixture changes.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_ide::{decode_tokens, legend_type_name, semantic_tokens};

/// A fixture exercising every legend token type: comment, keyword (`prefix` /
/// `from` / `@export`), namespace (prefix name + abs IRI), type (shape ref),
/// property (`ex:name`), function (stdlib call), number, string, operators
/// (`:=`, `=`, `|>`), field ref (`.name`), variable (mapping subject).
const FIXTURE: &str = "\
// a Fossil mapping
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")

User : ex:Person from users
    ex:name = upper(.name)
    ex:age = .age
";

fn render(src: &str) -> String {
    let system: Arc<dyn System> = Arc::new(NativeSystem);
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "fixture.fossil".to_string());
    let data = semantic_tokens(&db, file);
    let mut out = String::new();
    for (line, col, len, ty) in decode_tokens(&data) {
        out.push_str(&format!(
            "{line:>2}:{col:<2} len={len:<2} {}\n",
            legend_type_name(ty)
        ));
    }
    out
}

#[test]
fn semantic_tokens_snapshot() {
    insta::assert_snapshot!(render(FIXTURE));
}
