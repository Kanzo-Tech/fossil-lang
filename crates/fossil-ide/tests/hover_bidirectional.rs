//! Bidirectional hover integration test.
//!
//! Proves the hover feature end-to-end through the PUBLIC
//! [`fossil_ide::hover_bidirectional`] entry: hovering on a `users.field`
//! reference whose property key matches a target `ShEx` shape constraint
//! surfaces BOTH
//!
//!   (a) the **source-side** type, from the input descriptor the host
//!       registered for the row (forward propagation), AND
//!   (b) the **target-side** type, from the resolved `ShEx` shape constraint
//!       (`ShapeConstraint::value_ty`), reached by `resolve_target_shape`
//!       reading the document the PROGRAM names.
//!
//! Three cases:
//!   1. The program names a document: hover shows BOTH the source-side
//!      (`Integer`, from the input descriptor) and the target-side (`Float`,
//!      from `ShEx`) type, with the target block carrying the `ShEx` tagline.
//!   2. The program names none: hover shows the source-side block ONLY — no
//!      target block, no error (the "if reachable" hedge).
//!   3. No `Unknown` literal leaks into either output.
//!
//! Case 2 is decided by the program alone — there is no host mode that can turn
//! backward checking on or off behind it. That is the whole of F4.
//!
//! # The db stand-in (`HostDb`)
//!
//! A `#[salsa::db]` struct carrying a host `System` that reads the disk AND
//! installs the `ShEx` decoder row, plus — in [`fixture`] — the registration of
//! the document the program names. That is what a host owes the checker: a
//! filesystem, a decoder table, and the documents put in the database before
//! the queries look for them. It holds no descriptor of its own.

#![cfg(not(target_arch = "wasm32"))]
// The `.fossil` fixture sources interpolate — `"…/{users.id}"` is LITERAL
// Fossil source, not a Rust format-string arg.
#![allow(clippy::literal_string_with_formatting_args)]

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use fossil_base::{Catalogue, Files, FsError, Provider, SourceFile, System};

/// The test's host `System` — a filesystem plus the `ShEx` decoder row, the
/// same pair `fossil-lsp`'s `LspSystem` installs. `fossil_base::test_support::NativeSystem`
/// is not enough: its decoder table is the trait default `&[]`, and a document
/// nothing decodes resolves no shape.
#[derive(Debug, Default)]
struct HostSystem(fossil_descriptors_input::DescriptorCache);

impl System for HostSystem {
    /// The introspected-schema table. The trait default is `None`, and a host
    /// that keeps it hands these tests no typed source row at all.
    fn descriptors(&self) -> Option<&fossil_descriptors_input::DescriptorCache> {
        Some(&self.0)
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| FsError::Io(e.to_string()))
    }
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}

/// A host db stand-in: a real Salsa db (so tracked queries run) over a
/// [`HostSystem`] (so the documents the program names are readable).
#[salsa::db]
#[derive(Clone)]
struct HostDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
    catalogue: Catalogue,
}

impl std::fmt::Debug for HostDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDb").finish_non_exhaustive()
    }
}

#[salsa::db]
impl salsa::Database for HostDb {}

#[salsa::db]
impl fossil_base::Db for HostDb {
    fn system(&self) -> &dyn System {
        &*self.system
    }
    fn files(&self) -> &Files {
        &self.files
    }

    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }
}

impl HostDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            system: Arc::new(HostSystem::default()),
            files: Files::default(),
            catalogue: Catalogue::default(),
        }
    }
}

/// The introspected `users` row, registered by the HOST before the compile —
/// what `fossil_introspect::pre_introspect_and_register` and the browser's
/// `registerInferredDescriptor` do for real. Its `name` column is
/// `Integer`; see `SHEX_SRC` for why it is not `String` any more.
fn register_users(db: &dyn fossil_base::Db) {
    use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
    use fossil_graph_schema::Primitive;
    let Some(cache) = db.system().descriptors() else {
        panic!("the host keeps no descriptor table");
    };
    cache.insert(InferredDescriptor {
        uri: "users.csv".into(),
        columns: vec![
            InferredColumn {
                name: "id".into(),
                primitive: Primitive::Integer,
            },
            InferredColumn {
                name: "name".into(),
                primitive: Primitive::Integer,
            },
        ],
        freshness_token: String::new(),
    });
}

/// A `ShEx` schema declaring `http://example.org/Person` with an
/// `http://example.org/name` triple constraint narrowed to `xsd:float` —
/// DELIBERATELY different from the source-side `Integer`, so the two type blocks
/// are visibly distinct (source renders `Integer`, target renders `Float`).
/// The property key `name` is what binds the two: a bare key is the last segment
/// of a predicate IRI **the document declares**.
///
/// The pair used to be `String` against `Integer`, which is not merely distinct
/// but INCOMPATIBLE. Now that `resolve_target_shape` reads the document the
/// program names, the mismatch is a real error, the expression
/// stops typing, and hover has no type to show. An unchecked mismatch is no
/// longer a state this language can be in.
///
/// `Integer` against `Float` keeps the two blocks distinct AND well-typed, via
/// `S-IntFlt` — the subtyping rule kept precisely so
/// an `Integer` column can feed an `xsd:float` property without a hand-written
/// conversion. Nothing else in the suite exercises it.
const SHEX_SRC: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "TripleConstraint",
          "predicate": "http://example.org/name",
          "valueExpr": {
            "type": "NodeConstraint",
            "datatype": "http://www.w3.org/2001/XMLSchema#float"
          }
        }
      }
    }
  ]
}"#;

/// Build a `.fossil` source whose `users` row the host has introspected (so
/// source-side `users.name` resolves to `Integer`) and whose mapping targets
/// `Person` (so target-side resolution finds the `ShEx` shape). The shape
/// document is written next to the `.fossil`, because it is read by relative
/// path from the program.
fn fixture(dir: &std::path::Path) -> (HostDb, SourceFile) {
    // The shape document sits beside the program, and the PROGRAM names it:
    // `resolve_target_shape` reads what the program declares, so
    // hover resolves a target type for the same reason the compiler does.
    std::fs::write(dir.join("person.shex"), SHEX_SRC).expect("write ShEx");
    let fossil_path = dir.join("person.fossil");
    let src = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"users.csv\")
User : Person from users
    @subject = \"https://example.org/u/{users.id}\"
    name = users.name
";
    let mut db = HostDb::new();
    register_users(&db);
    let file = SourceFile::new(
        &db,
        src.to_string(),
        fossil_path.to_string_lossy().into_owned(),
    );
    // The host's half: the shape document is a Salsa input, so it has to be in
    // the database before any query goes looking for it. The `users` row came
    // in through the descriptor cache above — the input side has not moved.
    fossil_ide::register_missing_documents(&mut db, file, &|key| std::fs::read_to_string(key).ok());
    (db, file)
}

/// Hover position for the `name` property on line 4 (`    name = users.name`).
/// Column 14 is inside the `users.name` RHS value. Line 4 and not 3 because the
/// program carries the `type { Person } := io.shex(...)` line that names its
/// output document.
const NAME_LINE: u32 = 4;

/// The same position in the fixture that names NO document, which is one line
/// shorter.
const NAME_LINE_NO_DOCUMENT: u32 = 3;
const NAME_COL: u32 = 14;

/// Case 1 — the `Some`-shape path: hover shows BOTH the source-side
/// (`Integer`) AND the target-side (`ShEx` `Float`) type, target block tagged.
#[test]
fn hover_shows_source_and_target_type_when_shape_resolves() {
    let tmp = std::env::temp_dir().join(format!("fossil-hover-bidi-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mk tmp");
    let (db, file) = fixture(&tmp);

    let info = fossil_ide::hover_bidirectional(&db, file, NAME_LINE, NAME_COL)
        .expect("hover on `users.name` with a registered descriptor must return Some");
    let md = &info.markdown;

    // (a) source-side: the introspected `name` column is `Integer`.
    assert!(
        md.contains("Integer"),
        "hover must show the source-side type `Integer`; got {md:?}",
    );
    // (b) target-side: the ShEx constraint narrows `name` to `Float`. The
    //     pair is well-typed by `S-IntFlt`, which is why hover has anything to
    //     show at all — see the SHEX_SRC doc comment.
    assert!(
        md.contains("Float"),
        "hover must show the target-side ShEx type `Float`; got {md:?}",
    );
    // (c) the target block is explicitly tagged so the user knows its origin.
    assert!(
        md.contains("target type (ShEx shape constraint)"),
        "the target-side block must carry the ShEx tagline; got {md:?}",
    );
    // Two fenced blocks (source + target).
    assert_eq!(
        md.matches("```fossil").count(),
        2,
        "expected two fenced fossil blocks (source + target); got {md:?}",
    );
    // (3) No internal inference state leaks into the hover.
    assert!(
        !md.contains("Unknown") && !md.contains("InferenceId"),
        "hover leaked internal type state; got {md:?}",
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Case 2 — the program names no output document: hover shows the source-side
/// block ONLY (no target block, no error — the "if reachable" hedge). What
/// decides it is the program, and nothing else.
#[test]
fn hover_shows_source_only_when_the_program_names_no_document() {
    let tmp = std::env::temp_dir().join(format!("fossil-hover-accept-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mk tmp");

    // Same fixture, minus the `type { … } := io.shex(…)` line.
    let fossil_path = tmp.join("person.fossil");
    let src = "\
users := io.csv(\"users.csv\")
User : Person from users
    @subject = \"https://example.org/u/{users.id}\"
    name = users.name
";
    let db = HostDb::new();
    register_users(&db);
    let file = SourceFile::new(
        &db,
        src.to_string(),
        fossil_path.to_string_lossy().into_owned(),
    );

    let info = fossil_ide::hover_bidirectional(&db, file, NAME_LINE_NO_DOCUMENT, NAME_COL)
        .expect("hover on `users.name` still returns the source-side type with no output contract");
    let md = &info.markdown;

    // Source-side present — the input descriptor still types the column.
    assert!(
        md.contains("Integer"),
        "hover must still show the source-side `Integer`; got {md:?}",
    );
    // No target block, no ShEx tagline: the program declared no contract.
    assert!(
        !md.contains("target type (ShEx shape constraint)"),
        "a program naming no document must get NO target-side block; got {md:?}",
    );
    // Exactly one fenced block.
    assert_eq!(
        md.matches("```fossil").count(),
        1,
        "with no output contract hover has a single (source-only) block; got {md:?}",
    );
    assert!(!md.contains("Unknown") && !md.contains("InferenceId"));

    let _ = std::fs::remove_dir_all(&tmp);
}
