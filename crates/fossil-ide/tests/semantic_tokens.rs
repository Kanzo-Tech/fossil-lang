//! Snapshot test for `fossil_ide::semantic_tokens` (LSP-01 / SC#4).
//!
//! The raw LSP output is a flat `Vec<u32>` of delta-encoded 5-tuples — opaque to
//! review. We decode it back to absolute `(line, col, len, type)` rows and render
//! each with its human-readable legend type name, so the snapshot diffs as a
//! readable table when the classifier or fixture changes.

#![cfg(not(target_arch = "wasm32"))]
// The fixture interpolates — `"…/{users.id}"` is LITERAL Fossil source, not a
// Rust format-string arg.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_ide::{decode_tokens, legend_type_name, semantic_tokens};

/// A fixture exercising every legend token type the classifier can still
/// produce: comment, keyword (`from` and the `@subject` sigil), function (a
/// bare call callee), property (the member-access side of `users.name`), number,
/// string (including the parts an interpolation carves the literal into),
/// operators (`:=`, `=`, and the interpolation's `{` AND `}`) and variable (a
/// mapping subject, a binding, a shape name).
///
/// The closer is named because it was the one this snapshot recorded missing:
/// the row at `6:47` did not exist, so the committed table showed a literal
/// opening as an operator and ending in a gap — string, string, operator,
/// expression, nothing, string. It is the `RBRACE` arm of `semantic.rs` that
/// puts it there, and this table is where its absence was legible.
///
/// Two legend entries have no fixture and cannot get one: NAMESPACE and TYPE.
/// The first was the prefix segment of `ex:Person` plus the `<…>` absolute IRI,
/// and the second the shape ref — `semantic.rs` deleted the rules for both when
/// the CURIE and the IRI literal left the grammar, and a shape name is now an
/// ordinary IDENT that falls through to variable. They stay in the legend
/// because the legend is a wire index, and dropping an entry renumbers the ones
/// after it.
const FIXTURE: &str = "\
// a Fossil mapping
type { Person } := io.shex(\"person.shex\")

users := io.csv(\"users.csv\")

User : Person from users
    @subject = \"https://example.org/u/{users.id}\"
    name = upper(users.name)
    age = 42
";

fn render(src: &str) -> String {
    use std::fmt::Write as _;
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "fixture.fossil".to_string());
    let data = semantic_tokens(&db, file);
    let mut out = String::new();
    for (line, col, len, ty) in decode_tokens(&data) {
        let _ = writeln!(
            out,
            "{line:>2}:{col:<2} len={len:<2} {}",
            legend_type_name(ty)
        );
    }
    out
}

#[test]
fn semantic_tokens_snapshot() {
    insta::assert_snapshot!(render(FIXTURE));
}
