//! SC#4 (LSP-01 completion half) — three-source completion integration test.
//!
//! Proves [`fossil_ide::completions`] merges the three SC#4 sources end-to-end:
//!
//!   (a) a stdlib completion for an un-imported namespace carries a non-empty
//!       `additional_text_edits` (the gleam-lsp auto-import edit);
//!   (b) a `NativeUdfOnly` entry is tagged so the playground can gray it out;
//!   (c) a declared prefix appears as a completion;
//!   (d) when the cursor's mapping resolves a target `ShEx` shape, the shape's
//!       predicate names appear as completions.
//!
//! Case (d) needs the `HirDb` descriptor wiring (06-01 / ADR-0020 R2), so the
//! test builds a `ShExHostDb` host stand-in mirroring the production host-wrapper
//! contract (the same pattern as `tests/hover_bidirectional.rs`). Cases (a)-(c)
//! only need the stdlib catalog + the cross-file prefix index, which a degraded
//! `AcceptAll` host already provides.

#![cfg(not(target_arch = "wasm32"))]
// The `.fossil` fixtures contain `${ex:}` / `${.id}` template placeholders —
// LITERAL Fossil source, not Rust format-string args.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::{Files, NativeSystem, SourceFile, System};
use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_hir::HirDb;
use lsp_types::{CompletionItemKind, CompletionItemTag};

/// A host db stand-in carrying a host-supplied output descriptor, returned via
/// the `HirDb` override (mirrors the production host-wrapper contract).
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

/// A `ShEx` schema declaring `ex:Person` (`http://example.org/Person`) with a
/// `ex:name` triple constraint narrowed to `xsd:integer`.
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

const SRC: &str = "\
prefix ex: <http://example.org/>
User : ex:Person from users
    ex:name = .name
";

fn shex_db() -> ShExHostDb {
    let shex = ShExDescriptor::from_reader(SHEX_SRC.as_bytes()).expect("ShEx parses");
    ShExHostDb::new(OutputDescriptorKind::ShEx(shex))
}

fn file(db: &ShExHostDb, src: &str) -> SourceFile {
    SourceFile::new(db, src.to_string(), "complete.fossil".to_string())
}

/// (a) A stdlib completion for an un-imported namespace carries a non-empty
///     auto-import `additional_text_edits` (the gleam-lsp pattern).
#[test]
fn stdlib_completion_for_unimported_namespace_has_auto_import_edit() {
    let db = shex_db();
    let f = file(&db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, 14);

    let trim = items
        .iter()
        .find(|i| i.label == "clean.trim")
        .expect("clean.trim must be offered");
    assert_eq!(trim.kind, Some(CompletionItemKind::FUNCTION));
    let edits = trim
        .additional_text_edits
        .as_ref()
        .expect("an un-imported namespace must carry an auto-import edit");
    assert!(!edits.is_empty(), "auto-import edit must be non-empty");
    assert!(
        edits[0].new_text.contains("use clean"),
        "the auto-import edit must insert `use clean`; got {:?}",
        edits[0].new_text,
    );
}

/// (b) A `NativeUdfOnly` entry is tagged DEPRECATED so the playground can gray
///     it out.
#[test]
fn native_only_stdlib_entry_is_tagged() {
    let db = shex_db();
    let f = file(&db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, 14);

    // `clean.slug` lowers to a Rust UDF → NativeUdfOnly.
    let slug = items
        .iter()
        .find(|i| i.label == "clean.slug")
        .expect("clean.slug must be offered");
    assert_eq!(
        slug.tags.as_deref(),
        Some(&[CompletionItemTag::DEPRECATED][..]),
        "a NativeUdfOnly entry must be tagged",
    );
}

/// (c) A declared prefix appears as a completion.
#[test]
fn declared_prefix_is_offered() {
    let db = shex_db();
    let f = file(&db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, 14);

    assert!(
        items.iter().any(|i| i.label == "ex:"),
        "the declared `ex` prefix must be offered; labels = {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>(),
    );
}

/// (d) When the cursor's mapping resolves a target `ShEx` shape, the shape's
///     predicate names appear as Field completions.
#[test]
fn shape_property_names_are_offered_when_shape_resolves() {
    let db = shex_db();
    let f = file(&db, SRC);
    // Line 2 (`    ex:name = .name`) is inside the `User : ex:Person` mapping
    // whose target shape resolves to `ex:Person`; column 14 is inside the body.
    let items = fossil_ide::completions(&db, &[f], f, 2, 14);

    let shape_prop = items
        .iter()
        .find(|i| i.kind == Some(CompletionItemKind::FIELD))
        .expect("a resolved target shape must contribute Field (shape-property) completions");
    assert_eq!(
        shape_prop.label, "http://example.org/name",
        "the shape's `ex:name` predicate IRI must be offered as a property; got {:?}",
        shape_prop.label,
    );
    // Risk Register: the rendered detail must not leak internal type state.
    let detail = shape_prop.detail.as_deref().unwrap_or("");
    assert!(
        !detail.contains("Unknown") && !detail.contains("InferenceId"),
        "shape-property detail leaked internal type state; got {detail:?}",
    );
}

/// Under `AcceptAll` (no descriptor) the shape-property source contributes
/// nothing, but stdlib + prefixes still do — the "if reachable" hedge.
#[test]
fn accept_all_yields_no_shape_properties_but_keeps_stdlib() {
    let db = ShExHostDb::new(OutputDescriptorKind::ACCEPT_ALL_DEFAULT);
    let f = file(&db, SRC);
    let items = fossil_ide::completions(&db, &[f], f, 2, 14);

    assert!(
        !items
            .iter()
            .any(|i| i.kind == Some(CompletionItemKind::FIELD)),
        "AcceptAll must contribute no shape-property (Field) completions",
    );
    assert!(
        items.iter().any(|i| i.label == "clean.trim"),
        "stdlib completions must still be offered under AcceptAll",
    );
}
