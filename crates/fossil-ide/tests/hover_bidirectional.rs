//! SC#4 (LSP-01 hover half) — bidirectional hover integration test.
//!
//! Proves the headline Phase-6 LSP feature end-to-end through the PUBLIC
//! [`fossil_ide::hover_bidirectional`] entry: hovering on a `.field` reference
//! whose predicate matches a target `ShEx` shape constraint surfaces BOTH
//!
//!   (a) the **source-side** type, from the CSVW input descriptor (forward
//!       propagation — `resolve_source_row` reads the `schema = "<path>"`
//!       CSVW file via the host filesystem), AND
//!   (b) the **target-side** type, from the resolved `ShEx` shape constraint
//!       (`ShapeConstraint::value_ty`), now reachable via the 06-01 / ADR-0020
//!       R2 Db-wiring (`HirDb::output_descriptor_kind` → `resolve_target_shape`
//!       returns `Some`).
//!
//! Three cases:
//!   1. `Some`-shape: hover shows BOTH the source-side (`String`) and the
//!      target-side (ShEx) type, with the target block carrying the ShEx
//!      tagline.
//!   2. `AcceptAll`: hover shows the source-side block ONLY — no target block,
//!      no error (the "if reachable" hedge).
//!   3. No `Unknown` literal leaks into either output.
//!
//! # The host-wiring stand-in (`ShExHostDb`)
//!
//! `hover_bidirectional` takes `&dyn fossil_hir::HirDb` so it can read the
//! host's `output_descriptor_kind()`. A production host (the Phase-6 LSP db
//! wrapper) implements `HirDb` on its concrete `Db` type, returning a reference
//! into its own descriptor storage. This test builds a minimal such host db:
//! a `#[salsa::db]` struct carrying a `NativeSystem` (so the CSVW schema file
//! is readable) plus an `Arc<OutputDescriptorKind>` it returns from the
//! `HirDb` override. This is exactly the wiring contract documented in
//! `fossil_hir::db_ext`.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::{Files, NativeSystem, SourceFile, System};
use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_hir::HirDb;

/// A host db stand-in: a real Salsa db (so tracked queries run) that also
/// carries a host-supplied output descriptor, returned via the `HirDb`
/// override. Mirrors the production host-wrapper contract (see
/// `fossil_hir::db_ext`).
#[salsa::db]
#[derive(Clone)]
struct ShExHostDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
    descriptor: Arc<OutputDescriptorKind>,
}

impl std::fmt::Debug for ShExHostDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShExHostDb").finish_non_exhaustive()
    }
}

#[salsa::db]
impl salsa::Database for ShExHostDb {}

#[salsa::db]
impl fossil_base::Db for ShExHostDb {
    fn system(&self) -> &dyn System {
        &*self.system
    }
    fn files(&self) -> &Files {
        &self.files
    }
}

impl HirDb for ShExHostDb {
    fn output_descriptor_kind(&self) -> &OutputDescriptorKind {
        &self.descriptor
    }
}

impl ShExHostDb {
    fn new(descriptor: OutputDescriptorKind) -> Self {
        Self {
            storage: salsa::Storage::default(),
            system: Arc::new(NativeSystem),
            files: Files::default(),
            descriptor: Arc::new(descriptor),
        }
    }
}

/// CSVW schema declaring a `name` column typed `xsd:string`.
const USERS_CSVW: &str = r#"{
  "@context": "http://www.w3.org/ns/csvw",
  "url": "users.csv",
  "tableSchema": {
    "columns": [
      { "name": "id", "datatype": "string" },
      { "name": "name", "datatype": "string" }
    ]
  }
}"#;

/// A `ShEx` schema declaring `ex:Person` (full IRI `http://example.org/Person`)
/// with a `ex:name` triple constraint narrowed to `xsd:integer` — DELIBERATELY
/// different from the CSVW source-side `String`, so the two type blocks are
/// visibly distinct (the source side renders `String`, the target side renders
/// `Integer`).
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
            "datatype": "http://www.w3.org/2001/XMLSchema#integer"
          }
        }
      }
    }
  ]
}"#;

/// Build a `.fossil` source whose `users` source declares the CSVW schema
/// (so source-side `.name` resolves to `String`) and whose mapping targets
/// `ex:Person` (so target-side resolution finds the ShEx shape). Write the
/// CSVW file next to the `.fossil` so `resolve_source_row`'s relative-path
/// read succeeds.
fn fixture(dir: &std::path::Path) -> (ShExHostDb, SourceFile, OutputDescriptorKind) {
    std::fs::write(dir.join("users.csvw"), USERS_CSVW).expect("write CSVW");
    let fossil_path = dir.join("person.fossil");
    let src = "\
prefix ex: <http://example.org/>
users := io.csv(\"users.csv\", schema = \"users.csvw\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";
    let shex = ShExDescriptor::from_reader(SHEX_SRC.as_bytes()).expect("ShEx parses");
    let kind = OutputDescriptorKind::ShEx(shex);
    // The db OWNS one descriptor; we hand back a SECOND independent ShEx
    // descriptor for callers that want to assert on it (cheap to re-parse).
    let owned = OutputDescriptorKind::ShEx(
        ShExDescriptor::from_reader(SHEX_SRC.as_bytes()).expect("ShEx parses"),
    );
    let db = ShExHostDb::new(owned);
    let file = SourceFile::new(
        &db,
        src.to_string(),
        fossil_path.to_string_lossy().into_owned(),
    );
    (db, file, kind)
}

/// Hover position for `.name` on line 4 (`    ex:name = .name`). Column 14 is
/// inside the `.name` RHS value of the property.
const NAME_LINE: u32 = 4;
const NAME_COL: u32 = 14;

/// Case 1 — the `Some`-shape path: hover shows BOTH the source-side (CSVW
/// `String`) AND the target-side (ShEx `Integer`) type, target block tagged.
#[test]
fn hover_shows_source_and_target_type_when_shape_resolves() {
    let tmp = std::env::temp_dir().join(format!("fossil-hover-bidi-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mk tmp");
    let (db, file, _kind) = fixture(&tmp);

    let info = fossil_ide::hover_bidirectional(&db, file, NAME_LINE, NAME_COL)
        .expect("hover on `.name` with a CSVW schema must return Some");
    let md = &info.markdown;

    // (a) source-side: the CSVW `name` column is `String`.
    assert!(
        md.contains("String"),
        "hover must show the source-side CSVW type `String`; got {md:?}",
    );
    // (b) target-side: the ShEx constraint narrows `ex:name` to `Integer`.
    assert!(
        md.contains("Integer"),
        "hover must show the target-side ShEx type `Integer`; got {md:?}",
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
    // (3) Risk Register: no internal inference state leaks.
    assert!(
        !md.contains("Unknown") && !md.contains("InferenceId"),
        "hover leaked internal type state; got {md:?}",
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Case 2 — `AcceptAll`: hover shows the source-side block ONLY (no target
/// block, no error — the "if reachable" hedge).
#[test]
fn hover_shows_source_only_under_accept_all() {
    let tmp = std::env::temp_dir().join(format!("fossil-hover-accept-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mk tmp");

    // Same fixture, but the host db carries the degraded AcceptAll default.
    std::fs::write(tmp.join("users.csvw"), USERS_CSVW).expect("write CSVW");
    let fossil_path = tmp.join("person.fossil");
    let src = "\
prefix ex: <http://example.org/>
users := io.csv(\"users.csv\", schema = \"users.csvw\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";
    let db = ShExHostDb::new(OutputDescriptorKind::ACCEPT_ALL_DEFAULT);
    let file = SourceFile::new(
        &db,
        src.to_string(),
        fossil_path.to_string_lossy().into_owned(),
    );

    let info = fossil_ide::hover_bidirectional(&db, file, NAME_LINE, NAME_COL)
        .expect("hover on `.name` still returns the source-side type under AcceptAll");
    let md = &info.markdown;

    // Source-side present.
    assert!(
        md.contains("String"),
        "AcceptAll hover must still show the source-side `String`; got {md:?}",
    );
    // No target block, no ShEx tagline.
    assert!(
        !md.contains("target type (ShEx shape constraint)"),
        "AcceptAll hover must NOT append a target-side block; got {md:?}",
    );
    // Exactly one fenced block.
    assert_eq!(
        md.matches("```fossil").count(),
        1,
        "AcceptAll hover must have a single (source-only) fenced block; got {md:?}",
    );
    assert!(!md.contains("Unknown") && !md.contains("InferenceId"));

    let _ = std::fs::remove_dir_all(&tmp);
}
