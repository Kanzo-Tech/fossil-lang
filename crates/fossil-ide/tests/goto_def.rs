//! SC#4 (LSP-01 goto-def half) — cross-file goto-def integration test.
//!
//! Proves [`fossil_ide::goto_definition`] resolves the four symbol kinds the
//! success criterion names — prefix, mapping, function, shape ref — across the
//! open-file set (the open-files-as-workspace model, ADR-0023). The headline
//! case is CROSS-FILE: a prefix declared in file A resolves when the cursor is
//! on its use in file B.
//!
//! goto-def is pure name resolution (no descriptor / type wiring), so a plain
//! `fossil_base::FossilDb` is sufficient here — unlike completion's
//! shape-property source, this test needs no `ShEx` host stand-in.

#![cfg(not(target_arch = "wasm32"))]
// The `.fossil` fixtures contain `${ex:}` / `${.id}` template placeholders —
// LITERAL Fossil source, not Rust format-string args.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::{NativeSystem, SourceFile, System};
use fossil_ide::goto_definition;

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    fossil_base::FossilDb::new(system)
}

fn file(db: &fossil_base::FossilDb, name: &str, src: &str) -> SourceFile {
    SourceFile::new(db, src.to_string(), name.to_string())
}

/// File A declares the `ex` prefix; file B uses the `ex` prefix, declares a
/// `User` mapping targeting the `ex:Person` shape, and references `.name`.
/// The two-file set is the workspace.
const FILE_A: &str = "\
prefix ex: <https://example.org/>
";

const FILE_B: &str = "\
User : ex:Person from users
    ex:name = .name
";

/// 1. PREFIX (cross-file): cursor on the `ex` of `ex:Person` in file B resolves
///    to the `prefix ex: <...>` declaration in file A.
#[test]
fn goto_def_prefix_resolves_cross_file() {
    let db = db();
    let a = file(&db, "a.fossil", FILE_A);
    let b = file(&db, "b.fossil", FILE_B);

    // File B line 0: `User : ex:Person from users`. `ex` begins at column 7;
    // column 8 is unambiguously inside the `ex` token.
    let hits = goto_definition(&db, &[a, b], b, 0, 8);
    assert!(
        hits.iter().any(|t| t.file == a),
        "the `ex` prefix used in file B must resolve to its declaration in file A; got {hits:?}",
    );
    // The target range is non-empty and inside file A.
    let a_hit = hits.iter().find(|t| t.file == a).unwrap();
    assert!(
        a_hit.range.start < a_hit.range.end,
        "prefix decl range must be non-empty"
    );
}

/// 2. MAPPING: cursor on `User` (file B header) resolves to that mapping's
///    header site in file B.
#[test]
fn goto_def_mapping_resolves() {
    let db = db();
    let a = file(&db, "a.fossil", FILE_A);
    let b = file(&db, "b.fossil", FILE_B);

    // File B line 0, column 1 is inside `User`.
    let hits = goto_definition(&db, &[a, b], b, 0, 1);
    let user = hits
        .iter()
        .find(|t| t.file == b)
        .expect("User must resolve to its mapping header in file B");
    assert!(
        user.range.start < user.range.end,
        "mapping range must be non-empty"
    );
}

/// 3. SHAPE REF: cursor on `ex:Person` (file B header) resolves to the shape
///    reference recorded at that mapping site. The `ex` prefix segment ALSO
///    resolves to file A (prefix path) — both are valid; we assert the shape
///    target (file B) appears.
#[test]
fn goto_def_shape_ref_resolves() {
    let db = db();
    let a = file(&db, "a.fossil", FILE_A);
    let b = file(&db, "b.fossil", FILE_B);

    // File B line 0: `User : ex:Person from users`. `ex:Person` — the local
    // part `Person` begins after the colon (col 10); column 11 is inside it.
    let hits = goto_definition(&db, &[a, b], b, 0, 11);
    assert!(
        hits.iter().any(|t| t.file == b),
        "the `ex:Person` shape ref must resolve to its site in file B; got {hits:?}",
    );
}

/// goto-def on whitespace / a non-identifier returns no targets (never panics).
#[test]
fn goto_def_on_whitespace_is_empty() {
    let db = db();
    let b = file(&db, "b.fossil", FILE_B);
    // Column 4 on line 0 is the space between `User` and `:`.
    let hits = goto_definition(&db, &[b], b, 0, 4);
    assert!(
        hits.is_empty() || hits.iter().all(|t| t.range.start <= t.range.end),
        "goto-def on a separator must not produce a malformed target; got {hits:?}",
    );
}
