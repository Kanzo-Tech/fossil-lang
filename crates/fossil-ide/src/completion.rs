//! `textDocument/completion` — three sources.
//!
//! Merges three completion sources into a single `Vec<lsp_types::CompletionItem>`
//! (the `lsp-types`-direct shape is WASM-clean, so the browser host and the LSP
//! both consume it without a second conversion):
//!
//! 1. **stdlib functions**. Every [`fossil_hir::stdlib::RegistryEntry`] from
//!    [`FunctionRegistry::stdlib_default`] becomes a `CompletionItem` (kind =
//!    Function, detail = the rendered signature). There is no auto-import edit
//!    and no `(native-only)` tag: the first named a `use <ns>` line no program
//!    writes, and the second named `WasmClass::NativeUdfOnly`, a class that no
//!    longer exists.
//! 2. **shape properties** — when the cursor is in a mapping whose target `ShEx`
//!    shape resolves (the program names its output document with
//!    `type { … } := io.shex("…")`), the shape's `constraints[].predicate` names
//!    are offered as `Field` completions.
//! 3. **source fields** — the columns of the row the mapping reads, when the
//!    host has registered a descriptor for it.
//!
//! A source between the first and the second was **prefixes**: declared ones
//! from the cross-file index plus a well-known set offered as auto-importable.
//! It went whole with the `prefix` declaration; a vocabulary is not a form this
//! language has.
//!
//! # Domain + WASM boundary
//!
//! Returns `lsp_types::CompletionItem` directly; no stdio / JSON-RPC. The stdlib
//! source needs only the static catalog (`stdlib_default()`, no db); the
//! shape-property source calls `resolve_target_shape`, which reads the document
//! the program names as a Salsa INPUT — through `file_at` and the tracked
//! `shape_document`, which the HOST must have registered (see
//! [`crate::shape_documents`]). Both dependencies are file-keyed, not
//! per-mapping: ten mappings checked against one document share one decode, so
//! no new per-mapping key appears and the `body()` fan-out is unchanged
//! All type rendering routes through
//! [`fossil_hir::render_ty_kind`], so `TyKind::Unknown` never leaks into a
//! `detail` string: internal inference state must not reach the user.

use fossil_base::SourceFile;
use fossil_hir::def_map::def_map;
use fossil_hir::render_ty_kind;
use fossil_hir::shapes::resolve_target_shape;
use fossil_hir::stdlib::FunctionRegistry;
use fossil_hir::ty::TyKind;
use fossil_syntax::SyntaxKind;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemTag};

use crate::position::{node_at_position, token_at_position};

/// Compute completion items at an LSP position, merging the three sources.
///
/// `files` is the host's open-file set (for cross-file prefix resolution);
/// `file` is the file the cursor is in. A program that names no output shape
/// document simply contributes no shape-property items — the stdlib + prefix
/// sources are unconditional.
///
/// `line` / `character` are UTF-16 LSP coordinates.
#[must_use]
pub fn completions(
    db: &dyn fossil_base::Db,
    // The open-file set. It fed `prefix_completions`, the one source that was
    // cross-file; nothing left here reads past `file`, and the parameter stays
    // because it is `fossil-lsp`'s call shape and the next cross-file source
    // (shape names from a `type` binding in a sibling file) wants it back.
    _files: &[SourceFile],
    file: SourceFile,
    line: u32,
    character: u32,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    stdlib_completions(&mut items);
    shape_property_completions(db, file, line, character, &mut items);
    source_field_completions(db, file, line, character, &mut items);

    items
}

/// Source 1: stdlib functions with the gleam-lsp auto-import edit + native-only
/// tag.
fn stdlib_completions(items: &mut Vec<CompletionItem>) {
    let registry = FunctionRegistry::stdlib_default();
    for entry in registry.iter() {
        let name = entry.name.as_str();
        // The receiver is the dotted prefix (`str` in `str.trim`).
        let namespace = name.split('.').next().unwrap_or(name);

        // A `WasmClass::NativeUdfOnly` entry was tagged DEPRECATED here with a
        // `(native-only)` detail suffix, so the browser playground could gray
        // it out. There is no such entry and no such class: ruling 15 of
        // `SURFACE-PLAN.md` deleted the `Udf` lowering, and with it the only
        // reason a catalogued function could fail to run in a browser. Every
        // row runs everywhere, so nothing is grayed.
        let detail = render_sig(namespace, entry);
        let tags: Option<Vec<CompletionItemTag>> = None;

        // A gleam-lsp auto-import edit sat here: a top-of-file `use <ns>`
        // insertion, offered when the file's `PrefixIndex` said the namespace
        // was not declared. There is no `use` production — `use` is an ordinary
        // identifier and there is no module system for a name to come from —
        // and no prefix table to ask, and `io` / `str` / `clean` are resolved
        // by the checker against a catalogue rather than imported at all — so
        // the edit named a line no program writes.
        let additional_text_edits = None;

        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(detail),
            tags,
            additional_text_edits,
            ..Default::default()
        });
    }
}

/// Source 3: shape predicate names, when the enclosing mapping's target `ShEx`
/// shape resolves against the document the program names.
fn shape_property_completions(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
    items: &mut Vec<CompletionItem>,
) {
    let Some(mapping) = enclosing_mapping_loc(db, file, line, character) else {
        return;
    };
    // A document that is missing, undecodable or does not declare this shape is
    // a [`fossil_hir::shapes::TargetShapeError`], and `typecheck_mapping` is
    // where it becomes a diagnostic the user reads. A completion list is not a
    // place to report it: the honest answer here is to offer no shape
    // properties, which is what a program with no output contract gets too.
    let Ok(Some(shape)) = resolve_target_shape(db, mapping) else {
        return;
    };
    for constraint in &shape.constraints {
        let ty_str = constraint
            .value_ty
            .map_or_else(|| "Iri".to_string(), |ty| render_ty_kind(db, ty.kind(db)));
        items.push(CompletionItem {
            label: constraint.predicate.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(format!("shape property : {ty_str}")),
            ..Default::default()
        });
    }
}

/// Source 4: source-row field names, when the cursor is at a field reference
/// (`.<field>`) inside a mapping whose `from` source has a host-registered
/// `InferredDescriptor`. The fields + their inferred types come from
/// [`fossil_hir::infer::source_row_inferred`] — the same forward-typed record
/// the checker reads. Contributes nothing when no descriptor is registered (the
/// host did not pre-introspect the source): the editor stays quiet rather than
/// guessing field names.
fn source_field_completions(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
    items: &mut Vec<CompletionItem>,
) {
    if !at_field_ref_context(db, file, line, character) {
        return;
    }
    let Some(mapping) = enclosing_mapping_loc(db, file, line, character) else {
        return;
    };
    let Some(row) = fossil_hir::infer::source_row_inferred(db, mapping) else {
        return;
    };
    let TyKind::Record(record) = row.kind(db) else {
        return;
    };
    for field in record.fields(db) {
        items.push(CompletionItem {
            label: field.name.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(format!(
                "source field : {}",
                render_ty_kind(db, field.ty.kind(db))
            )),
            ..Default::default()
        });
    }
}

/// True when the cursor sits at a field reference — the `.` token itself
/// (completion triggered right after typing `.`) or anywhere inside a
/// `FIELD_REF_EXPR`. Bounded by the enclosing `MAPPING` so a stray
/// dot elsewhere does not fire source-field completion.
fn at_field_ref_context(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> bool {
    let Some(token) = token_at_position(db, file, line, character) else {
        return false;
    };
    if token.kind() == SyntaxKind::DOT {
        return true;
    }
    let mut current = token.parent();
    while let Some(node) = current {
        match node.kind() {
            // `SyntaxKind::FIELD_REF_EXPR => return true` was the first arm.
            // A leading `.` starts nothing, so the `DOT`
            // check above is the whole of the trigger — which is right for the
            // qualified form too: the cursor sits on the `.` of `User.` when
            // the completion is wanted.
            SyntaxKind::MAPPING => return false,
            _ => current = node.parent(),
        }
    }
    false
}

/// Resolve the cursor's enclosing mapping to its [`def_map`] `MappingLoc`
/// (mirrors `hover::resolve_hover_target`'s filter-then-nth contract,
/// without the property-level resolution).
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Salsa-handle lifetime contract
fn enclosing_mapping_loc<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<fossil_hir::def_map::MappingLoc<'db>> {
    let node = node_at_position(db, file, line, character)?;
    let mut current = Some(node);
    let mut mapping_node = None;
    while let Some(n) = current {
        if n.kind() == SyntaxKind::MAPPING {
            mapping_node = Some(n);
            break;
        }
        current = n.parent();
    }
    let mapping_node = mapping_node?;
    let cst = fossil_syntax::parse(db, file);
    let mapping_index = cst
        .root(db)
        .syntax()
        .children()
        .filter(|c| c.kind() == SyntaxKind::MAPPING)
        .position(|c| c == mapping_node)?;
    def_map(db, file)
        .mappings(db)
        .iter()
        .find(|m| m.index(db) == mapping_index)
        .copied()
}

/// Render a stdlib entry's signature as a `detail` string from its `'db`-free
/// [`fossil_hir::stdlib::SigSpec`] — never touches `TyKind::Unknown` (the catalog
/// carries only concrete scalar types).
fn render_sig(namespace: &str, entry: &fossil_hir::stdlib::RegistryEntry) -> String {
    let params = entry
        .sig
        .params
        .iter()
        .map(|p| scalar_name(p.ty))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = scalar_name(entry.sig.ret);
    format!("{}({params}) -> {ret}  [{namespace}]", entry.name)
}

/// Human-readable name for a `'db`-free [`fossil_hir::stdlib::ScalarTy`]. Mirrors
/// `render_ty_kind`'s surface names; never emits `Unknown`.
fn scalar_name(s: fossil_hir::stdlib::ScalarTy) -> String {
    use fossil_hir::stdlib::ScalarTy as S;
    match s {
        S::String => "String",
        S::Integer => "Integer",
        S::Float => "Float",
        S::Bool => "Bool",
        S::Date => "Date",
        S::DateTime => "DateTime",
        S::SeqString => "Seq<String>",
    }
    .to_string()
}

// `import_edit` and `prefix_import_edit` lived here, plus the `top_of_file`
// zero-width range both inserted at. One wrote `use <namespace>`, the other
// `prefix <p>: <iri>`; neither line is a production the language has.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use fossil_base::{Catalogue, Files, NativeSystem, System};
    use std::sync::Arc;

    // A minimal host db stand-in, so these unit tests exercise the stdlib +
    // prefix sources (which are unconditional) without a filesystem fixture.
    // The shape-property source needs a program naming a real document on
    // disk, and is exercised end-to-end in `tests/completion.rs`.
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

    fn db() -> HostDb {
        HostDb {
            storage: salsa::Storage::default(),
            system: Arc::new(NativeSystem::default()),
            files: Files::default(),
            catalogue: Catalogue::default(),
        }
    }

    fn file(db: &HostDb, src: &str) -> SourceFile {
        SourceFile::new(db, src.to_string(), "c.fossil".to_string())
    }

    /// The stdlib source is unconditional and BARE: every catalogued entry is
    /// offered wherever the cursor is, with no auto-import edit and no tag.
    ///
    /// Four tests stood here and all four asserted the opposite. Two wanted an
    /// `additional_text_edits` inserting `use clean` — there is no `use`
    /// production and no module system for a name to come from. One wanted a
    /// `DEPRECATED` tag and a `(native-only)` detail on `clean.slug`, from a
    /// `WasmClass::NativeUdfOnly` that no longer exists: every catalogued row
    /// runs everywhere now, so nothing is grayed. Two more wanted `ex:` and
    /// `xsd:` offered as prefix completions.
    #[test]
    fn stdlib_entries_are_offered_bare() {
        let db = db();
        let f = file(&db, "Users : Person from u\n    name = u.name\n");
        let items = completions(&db, &[f], f, 0, 0);
        let trim = items
            .iter()
            .find(|i| i.label == "str.trim")
            .expect("str.trim must be offered");
        assert_eq!(trim.kind, Some(CompletionItemKind::FUNCTION));
        assert!(
            trim.additional_text_edits.is_none(),
            "there is no import line to add; got {:?}",
            trim.additional_text_edits,
        );
        assert!(
            trim.tags.is_none(),
            "no entry is native-only any more; got {:?}",
            trim.tags,
        );
        assert!(
            !items.iter().any(|i| i.label.ends_with(':')),
            "a `prefix:` completion is a form this language does not have",
        );
    }

    /// Source 4: with a host-registered `InferredDescriptor`, a `.` field
    /// reference offers the source's columns as Field completions.
    #[test]
    fn offers_source_fields_after_dot_when_descriptor_registered() {
        use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
        let db = db();
        db.system
            .descriptors()
            .expect("the test host keeps a descriptor table")
            .insert(InferredDescriptor {
                // Keyed by the URI the binding names, not by `u` —
                // so the program below has to declare the binding for the
                // completion to find anything.
                uri: "u.csv".into(),
                columns: vec![
                    InferredColumn {
                        name: "name".into(),
                        primitive: fossil_graph_schema::Primitive::String,
                    },
                    InferredColumn {
                        name: "age".into(),
                        primitive: fossil_graph_schema::Primitive::Integer,
                    },
                ],
                freshness_token: String::new(),
            });
        // `ex:` must be declared so the mapping lowers (lower_mapping resolves
        // the shape prefix); the `.name` field ref sits on line 3.
        let src = "prefix ex: <https://example.org/>\nu := io.csv(\"u.csv\")\n\
                   User : ex:Person from u\n    ex:name = .name\n";
        let f = file(&db, src);
        // Cursor right after the `.` on line 3 → token_at_position picks the DOT.
        let dot = u32::try_from(src.lines().nth(3).unwrap().find('.').unwrap()).unwrap();
        let items = completions(&db, &[f], f, 3, dot + 1);
        let fields: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
            .map(|i| i.label.as_str())
            .collect();
        assert!(
            fields.contains(&"name"),
            "source field `name` must be offered at `.`; got {fields:?}",
        );
        assert!(
            fields.contains(&"age"),
            "source field `age` must be offered at `.`; got {fields:?}",
        );
    }

    /// Without a registered descriptor the source-field source stays quiet — no
    /// guessing field names (the editor degrades, not invents).
    #[test]
    fn no_source_fields_without_descriptor() {
        let db = db();
        let src =
            "prefix ex: <https://example.org/>\nUser : ex:Person from u\n    ex:name = .name\n";
        let f = file(&db, src);
        let dot = u32::try_from(src.lines().nth(2).unwrap().find('.').unwrap()).unwrap();
        let items = completions(&db, &[f], f, 2, dot + 1);
        assert!(
            !items.iter().any(|i| i.label == "name" || i.label == "age"),
            "no source fields should be offered without a descriptor",
        );
    }

    /// No `TyKind::Unknown` leaks into any `detail` string.
    #[test]
    fn no_unknown_leaks_into_detail() {
        let db = db();
        let f = file(&db, "User : ex:Person from u\n");
        let items = completions(&db, &[f], f, 0, 0);
        for i in &items {
            let d = i.detail.as_deref().unwrap_or("");
            assert!(
                !d.contains("Unknown") && !d.contains("InferenceId"),
                "completion detail leaked internal type state: {d:?}",
            );
        }
    }
}
