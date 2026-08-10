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
//!       (`ShapeConstraint::value_ty`), reached by `resolve_target_shape`
//!       reading the document the PROGRAM names (ADR-0055, F4).
//!
//! Three cases:
//!   1. The program names a document: hover shows BOTH the source-side
//!      (`Integer`, from CSVW) and the target-side (`Float`, from `ShEx`) type,
//!      with the target block carrying the `ShEx` tagline.
//!   2. The program names none: hover shows the source-side block ONLY — no
//!      target block, no error (the "if reachable" hedge).
//!   3. No `Unknown` literal leaks into either output.
//!
//! Case 2 used to be "under `AcceptAll`", a mode the HOST selected. The rule is
//! unchanged; what decides it moved to the program, which is the whole of F4.
//!
//! # The db stand-in (`ShExHostDb`)
//!
//! A `#[salsa::db]` struct carrying a `NativeSystem`, so the CSVW schema and the
//! `ShEx` document are both readable from disk. It still implements `HirDb` and
//! still holds an `OutputDescriptorKind`, and NEITHER is read any more —
//! `resolve_target_shape` stopped taking a descriptor. That dead host-descriptor
//! mechanism is a deletion of its own, not something to leave standing.

#![cfg(not(target_arch = "wasm32"))]
// The `.fossil` fixture sources contain `${ex:}` / `${.id}` template
// placeholders — LITERAL Fossil source, not Rust format-string args.
#![allow(clippy::literal_string_with_formatting_args)]

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
            system: Arc::new(NativeSystem::default()),
            files: Files::default(),
            descriptor: Arc::new(descriptor),
        }
    }
}

/// CSVW schema declaring a `name` column typed `xsd:integer` — see `SHEX_SRC`
/// for why it is not `xsd:string` any more.
const USERS_CSVW: &str = r#"{
  "@context": "http://www.w3.org/ns/csvw",
  "url": "users.csv",
  "tableSchema": {
    "columns": [
      { "name": "id", "datatype": "string" },
      { "name": "name", "datatype": "integer" }
    ]
  }
}"#;

/// A `ShEx` schema declaring `ex:Person` (full IRI `http://example.org/Person`)
/// with a `ex:name` triple constraint narrowed to `xsd:float` — DELIBERATELY
/// different from the CSVW source-side `Integer`, so the two type blocks are
/// visibly distinct (source renders `Integer`, target renders `Float`).
///
/// The pair used to be `String` against `Integer`, which is not merely distinct
/// but INCOMPATIBLE. That was harmless while the checker was handed
/// `ACCEPT_ALL_DEFAULT` and never looked; once `resolve_target_shape` reads the
/// document the program names (ADR-0055, F4), the mismatch is a real error, the
/// expression stops typing, and hover has no type to show. An unchecked
/// mismatch is no longer a state this language can be in.
///
/// `Integer` against `Float` keeps the two blocks distinct AND well-typed, via
/// `S-IntFlt` — the subtyping rule ADR-0057's fourth amendment kept precisely so
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

/// Build a `.fossil` source whose `users` source declares the CSVW schema
/// (so source-side `.name` resolves to `Integer`) and whose mapping targets
/// `ex:Person` (so target-side resolution finds the `ShEx` shape). Both the
/// CSVW file AND the shape document are written next to the `.fossil`, because
/// both are read by relative path from the program.
fn fixture(dir: &std::path::Path) -> (ShExHostDb, SourceFile, OutputDescriptorKind) {
    std::fs::write(dir.join("users.csvw"), USERS_CSVW).expect("write CSVW");
    // The shape document sits beside the program, and the PROGRAM names it.
    // It used to reach the checker from the host alone; `resolve_target_shape`
    // now reads what the program declares (ADR-0055, F4), so hover resolves a
    // target type for the same reason the compiler does.
    std::fs::write(dir.join("person.shex"), SHEX_SRC).expect("write ShEx");
    let fossil_path = dir.join("person.fossil");
    let src = "\
prefix ex: <http://example.org/>
type { Person } = io.shex(\"person.shex\")
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

/// Hover position for `.name` on line 5 (`    ex:name = .name`). Column 14 is
/// inside the `.name` RHS value of the property. Line 5 and not 4 because the
/// program now carries the `type { Person } = io.shex(...)` line that names its
/// output document.
const NAME_LINE: u32 = 5;

/// The same position in the fixture that names NO document, which is one line
/// shorter.
const NAME_LINE_NO_DOCUMENT: u32 = 4;
const NAME_COL: u32 = 14;

/// Case 1 — the `Some`-shape path: hover shows BOTH the source-side (CSVW
/// `String`) AND the target-side (`ShEx` `Integer`) type, target block tagged.
#[test]
fn hover_shows_source_and_target_type_when_shape_resolves() {
    let tmp = std::env::temp_dir().join(format!("fossil-hover-bidi-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mk tmp");
    let (db, file, _kind) = fixture(&tmp);

    let info = fossil_ide::hover_bidirectional(&db, file, NAME_LINE, NAME_COL)
        .expect("hover on `.name` with a CSVW schema must return Some");
    let md = &info.markdown;

    // (a) source-side: the CSVW `name` column is `Integer`.
    assert!(
        md.contains("Integer"),
        "hover must show the source-side CSVW type `Integer`; got {md:?}",
    );
    // (b) target-side: the ShEx constraint narrows `ex:name` to `Float`. The
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
    // (3) Risk Register: no internal inference state leaks.
    assert!(
        !md.contains("Unknown") && !md.contains("InferenceId"),
        "hover leaked internal type state; got {md:?}",
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Case 2 — the program names no output document: hover shows the source-side
/// block ONLY (no target block, no error — the "if reachable" hedge).
///
/// This used to be "under `AcceptAll`", a mode the HOST chose. The rule is
/// unchanged; what decides it moved to the program, which is ADR-0055's F4.
#[test]
fn hover_shows_source_only_when_the_program_names_no_document() {
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

    let info = fossil_ide::hover_bidirectional(&db, file, NAME_LINE_NO_DOCUMENT, NAME_COL)
        .expect("hover on `.name` still returns the source-side type with no output contract");
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
