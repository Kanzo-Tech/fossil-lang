//! Snapshot test for `fossil_ide::semantic_tokens` (LSP-01 / SC#4).
//!
//! The raw LSP output is delta-encoded indices — opaque to review. We decode it
//! back to absolute `(line, col, len, type)` rows and render each type through
//! the legend the server declares, so the snapshot diffs as a readable table
//! when the classifier or fixture changes, and reads the wire the way a client
//! does.

// The fixture interpolates — `"…/{users.id}"` is LITERAL Fossil source, not a
// Rust format-string arg.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_ide::{decode_tokens, modifier_names, semantic_legend, semantic_tokens};

/// A fixture exercising every legend token type: comment, keyword (`from`,
/// the `@subject` sigil, and the contextual `type`), namespace (the
/// catalogue head `io` and a `@connection` carved out of a reference), type
/// (a shape declared, named by a header, and applied as an edge), function (a
/// called member), property (a column and a body key), parameter (a named
/// argument), number, string (including the parts an interpolation carves the
/// literal into), operators (`:=`, `=`, and the interpolation's `{` AND `}`)
/// and variable (bindings, declared and used).
///
/// The `declaration` modifier renders as `+declaration` after the type.
const FIXTURE: &str = "\
// a Fossil mapping
type { Person } := io.shex(\"@shapes/person.shex\")

users := io.csv(\"users.csv\", delimiter = \";\")
admins := users.where(users.age >= 18)

User : Person from admins
    @subject = \"https://example.org/u/{users.id}\"
    name = str.upper(users.name)
    friend = Person(users.friend_id)
    age = 42
";

fn render(src: &str) -> String {
    use std::fmt::Write as _;
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "fixture.fossil".to_string());
    let legend = semantic_legend();
    let mut out = String::new();
    for (line, col, len, ty, mods) in decode_tokens(&semantic_tokens(&db, file)) {
        let mut kind = legend.token_types[ty as usize].as_str().to_string();
        for m in modifier_names(mods) {
            kind.push('+');
            kind.push_str(m);
        }
        let _ = writeln!(out, "{line:>2}:{col:<2} len={len:<2} {kind}");
    }
    out
}

#[test]
fn semantic_tokens_snapshot() {
    expect_test::expect_file!["snapshots/semantic_tokens_snapshot.txt"].assert_eq(&render(FIXTURE));
}
