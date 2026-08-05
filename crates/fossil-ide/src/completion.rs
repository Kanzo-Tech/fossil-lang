//! `textDocument/completion` — three-source completion (SC#4).
//!
//! Merges the three completion sources SC#4 requires into a single
//! `Vec<lsp_types::CompletionItem>` (the `lsp-types`-direct shape confirmed
//! WASM-clean by 06-01 Spike A, so the playground and the LSP both consume it
//! without a second conversion):
//!
//! 1. **stdlib functions** (the gleam-lsp auto-import pattern). Every
//!    [`fossil_registry::RegistryEntry`] from [`FunctionRegistry::stdlib_default`]
//!    becomes a `CompletionItem` (kind = Function, detail = the rendered
//!    signature). When the function's namespace prefix is NOT yet present in the
//!    file, the item carries an `additional_text_edits` insertion adding the
//!    `use <ns>` line at the top of the file — accepting the completion inserts
//!    BOTH the call AND the import in one step (Research §completion). A
//!    [`WasmClass::NativeUdfOnly`] entry is tagged `DEPRECATED` (the only
//!    `CompletionItemTag` in LSP 3.17) + a `(native-only)` detail suffix so the
//!    playground can gray it out.
//! 2. **prefixes** — declared prefixes from the cross-file
//!    [`crate::WorkspaceIndex`] + the well-known set
//!    ([`crate::WELL_KNOWN_PREFIXES`]: rdf/rdfs/xsd/owl), the latter
//!    offered as auto-importable (a `prefix <p>: <iri>` `additional_text_edits`
//!    insertion when not already declared).
//! 3. **shape properties** — when the cursor is in a mapping whose target `ShEx`
//!    shape resolves (the 06-01 / ADR-0020 R2 `HirDb` wiring), the shape's
//!    `constraints[].predicate` names are offered as `Field` completions.
//!
//! # Domain + WASM boundary
//!
//! Returns `lsp_types::CompletionItem` directly; no stdio / JSON-RPC. The stdlib
//! source needs only the static catalog (`stdlib_default()`, no db); the prefix
//! source needs the `WorkspaceIndex` (a CST-walk struct, no Salsa query); the
//! shape-property source reads `HirDb::output_descriptor_kind()` →
//! `resolve_target_shape` (the existing ADR-0020 accessor — NOT a new
//! per-mapping Salsa key). The per-mapping `body()` fan-out is unchanged
//! (Research Pitfall #3). All type rendering routes through
//! [`fossil_hir::render_ty_kind`], so `TyKind::Unknown` never leaks into a
//! `detail` string (Risk Register).

use crate::{PrefixIndex, WELL_KNOWN_PREFIXES, WorkspaceIndex};
use fossil_base::SourceFile;
use fossil_hir::HirDb;
use fossil_hir::def_map::def_map;
use fossil_hir::render_ty_kind;
use fossil_hir::shapes::resolve_target_shape;
use fossil_hir::ty::TyKind;
use fossil_registry::{FunctionRegistry, WasmClass};
use fossil_syntax::SyntaxKind;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemTag, Position, Range, TextEdit};

use crate::position::{node_at_position, token_at_position};

/// Compute completion items at an LSP position, merging the three SC#4 sources.
///
/// `files` is the host's open-file set (for cross-file prefix resolution);
/// `file` is the file the cursor is in. Takes `&dyn HirDb` (like
/// [`crate::hover_bidirectional`]) so the shape-property source can reach the
/// host descriptor; a host without a descriptor (the `AcceptAll` default) simply
/// contributes no shape-property items — the stdlib + prefix sources are
/// unconditional.
///
/// `line` / `character` are UTF-16 LSP coordinates.
#[must_use]
pub fn completions(
    db: &dyn HirDb,
    files: &[SourceFile],
    file: SourceFile,
    line: u32,
    character: u32,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let prefixes = PrefixIndex::build(db, file);

    stdlib_completions(&prefixes, &mut items);
    prefix_completions(db, files, &prefixes, &mut items);
    shape_property_completions(db, file, line, character, &mut items);
    source_field_completions(db, file, line, character, &mut items);

    items
}

/// Source 1: stdlib functions with the gleam-lsp auto-import edit + native-only
/// tag.
fn stdlib_completions(prefixes: &PrefixIndex, items: &mut Vec<CompletionItem>) {
    let registry = FunctionRegistry::stdlib_default();
    for entry in registry.iter() {
        let name = entry.name.as_str();
        // The namespace is the dotted prefix (`clean` in `clean.trim`).
        let namespace = name.split('.').next().unwrap_or(name);
        let native_only = entry.wasm_class == WasmClass::NativeUdfOnly;

        let mut detail = render_sig(namespace, entry);
        let tags = if native_only {
            detail.push_str("  (native-only)");
            Some(vec![CompletionItemTag::DEPRECATED])
        } else {
            None
        };

        // gleam-lsp auto-import: if the namespace prefix is not yet declared in
        // the file, attach a top-of-file `use <ns>` insertion so accepting the
        // completion also imports the namespace.
        let additional_text_edits = if prefixes.is_declared(namespace) {
            None
        } else {
            Some(vec![import_edit(namespace)])
        };

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

/// Source 2: declared (cross-file) prefixes + well-known prefixes (the latter
/// auto-importable when not yet declared).
fn prefix_completions(
    db: &dyn HirDb,
    files: &[SourceFile],
    local: &PrefixIndex,
    items: &mut Vec<CompletionItem>,
) {
    let ws = WorkspaceIndex::build(db, files);

    // Declared prefixes across the open-file set (cross-file). De-dup by name.
    let mut seen: Vec<String> = Vec::new();
    for f in ws.files() {
        let pidx = PrefixIndex::build(db, f);
        for binding in pidx.declared() {
            let p = binding.prefix.as_str();
            if seen.iter().any(|s| s == p) {
                continue;
            }
            seen.push(p.to_string());
            items.push(CompletionItem {
                label: format!("{p}:"),
                kind: Some(CompletionItemKind::MODULE),
                detail: Some(format!("prefix {p}: <{}>", binding.iri)),
                ..Default::default()
            });
        }
    }

    // Well-known prefixes: offered as auto-importable when not already declared.
    for (prefix, iri) in WELL_KNOWN_PREFIXES {
        if local.is_declared(prefix) || seen.iter().any(|s| s == prefix) {
            continue;
        }
        items.push(CompletionItem {
            label: format!("{prefix}:"),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(format!("well-known prefix <{iri}>")),
            additional_text_edits: Some(vec![prefix_import_edit(prefix, iri)]),
            ..Default::default()
        });
    }
}

/// Source 3: shape predicate names, when the enclosing mapping's target `ShEx`
/// shape resolves. Reads the host descriptor via the ADR-0020 accessor.
fn shape_property_completions(
    db: &dyn HirDb,
    file: SourceFile,
    line: u32,
    character: u32,
    items: &mut Vec<CompletionItem>,
) {
    let Some(mapping) = enclosing_mapping_loc(db, file, line, character) else {
        return;
    };
    let Some(shape) = resolve_target_shape(db, mapping, db.output_descriptor_kind()) else {
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
    db: &dyn HirDb,
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
/// `FIELD_REF` / `FIELD_REF_EXPR`. Bounded by the enclosing `MAPPING` so a stray
/// dot elsewhere does not fire source-field completion.
fn at_field_ref_context(db: &dyn HirDb, file: SourceFile, line: u32, character: u32) -> bool {
    let Some(token) = token_at_position(db, file, line, character) else {
        return false;
    };
    if token.kind() == SyntaxKind::DOT {
        return true;
    }
    let mut current = token.parent();
    while let Some(node) = current {
        match node.kind() {
            SyntaxKind::FIELD_REF_EXPR | SyntaxKind::FIELD_REF => return true,
            SyntaxKind::MAPPING => return false,
            _ => current = node.parent(),
        }
    }
    false
}

/// Resolve the cursor's enclosing mapping to its [`def_map`] `MappingLoc`
/// (mirrors `hover::resolve_hover_target`'s filter-then-nth contract per
/// ADR-0005, without the property-level resolution).
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Salsa-handle lifetime contract
fn enclosing_mapping_loc<'db>(
    db: &'db dyn HirDb,
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
/// [`fossil_registry::SigSpec`] — never touches `TyKind::Unknown` (the catalog
/// carries only concrete scalar types).
fn render_sig(namespace: &str, entry: &fossil_registry::RegistryEntry) -> String {
    let params = entry
        .sig
        .params
        .iter()
        .copied()
        .map(scalar_name)
        .collect::<Vec<_>>()
        .join(", ");
    let ret = scalar_name(entry.sig.ret);
    format!("{}({params}) -> {ret}  [{namespace}]", entry.name)
}

/// Human-readable name for a `'db`-free [`fossil_registry::ScalarTy`]. Mirrors
/// `render_ty_kind`'s surface names; never emits `Unknown`.
fn scalar_name(s: fossil_registry::ScalarTy) -> String {
    use fossil_registry::ScalarTy as S;
    match s {
        S::String => "String",
        S::Integer => "Integer",
        S::Float => "Float",
        S::Bool => "Bool",
        S::Date => "Date",
        S::DateTime => "DateTime",
        S::Iri => "Iri",
        S::TripleTerm => "TripleTerm",
        S::SeqString => "Seq<String>",
    }
    .to_string()
}

/// A top-of-file `use <namespace>` import edit (the gleam-lsp auto-import for an
/// un-imported stdlib namespace).
fn import_edit(namespace: &str) -> TextEdit {
    TextEdit::new(top_of_file(), format!("use {namespace}\n"))
}

/// A top-of-file `prefix <p>: <iri>` declaration edit (auto-import for a
/// well-known prefix).
fn prefix_import_edit(prefix: &str, iri: &str) -> TextEdit {
    TextEdit::new(top_of_file(), format!("prefix {prefix}: <{iri}>\n"))
}

/// The zero-width range at the very start of the file (line 0, char 0) — where
/// auto-import insertions land.
const fn top_of_file() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 0,
        },
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use fossil_base::{Files, NativeSystem, System};
    use std::sync::Arc;

    // A minimal `HirDb` host stand-in. `fossil-hir` only impls `HirDb` for
    // `FossilDb` under its own `#[cfg(test)]`, so the IDE crate supplies its own
    // host db. The default `output_descriptor_kind()` yields the degraded
    // `AcceptAll` fallback — sufficient for the stdlib + prefix sources (which
    // are unconditional); the shape-property source is exercised end-to-end
    // against a real `ShEx` descriptor in `tests/completion.rs`.
    #[salsa::db]
    #[derive(Clone)]
    struct HostDb {
        storage: salsa::Storage<Self>,
        system: Arc<dyn System>,
        files: Files,
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
    }

    impl HirDb for HostDb {}

    fn db() -> HostDb {
        HostDb {
            storage: salsa::Storage::default(),
            system: Arc::new(NativeSystem::default()),
            files: Files::default(),
        }
    }

    fn file(db: &HostDb, src: &str) -> SourceFile {
        SourceFile::new(db, src.to_string(), "c.fossil".to_string())
    }

    /// Under `AcceptAll` there are no shape properties, but the stdlib + prefix
    /// sources are unconditional.
    #[test]
    fn offers_stdlib_with_auto_import_for_unimported_namespace() {
        let db = db();
        // No `clean` namespace declared → the `clean.*` items must carry an
        // auto-import edit.
        let f = file(&db, "User : ex:Person from u\n    ex:name = .name\n");
        let items = completions(&db, &[f], f, 0, 0);
        let trim = items
            .iter()
            .find(|i| i.label == "clean.trim")
            .expect("clean.trim must be offered");
        assert_eq!(trim.kind, Some(CompletionItemKind::FUNCTION));
        let edits = trim
            .additional_text_edits
            .as_ref()
            .expect("un-imported namespace must carry an auto-import edit");
        assert!(
            edits[0].new_text.contains("use clean"),
            "auto-import edit must insert `use clean`; got {:?}",
            edits[0].new_text,
        );
    }

    #[test]
    fn native_only_entries_are_tagged() {
        let db = db();
        let f = file(&db, "User : ex:Person from u\n");
        let items = completions(&db, &[f], f, 0, 0);
        // `clean.slug` lowers to a Rust UDF → NativeUdfOnly.
        let slug = items
            .iter()
            .find(|i| i.label == "clean.slug")
            .expect("clean.slug must be offered");
        assert_eq!(
            slug.tags.as_deref(),
            Some(&[CompletionItemTag::DEPRECATED][..]),
            "a NativeUdfOnly entry must be tagged so the playground can gray it",
        );
        assert!(
            slug.detail.as_deref().unwrap_or("").contains("native-only"),
            "native-only entry detail must say so; got {:?}",
            slug.detail,
        );
    }

    #[test]
    fn offers_declared_prefix() {
        let db = db();
        let f = file(&db, "prefix ex: <https://example.org/>\n");
        let items = completions(&db, &[f], f, 0, 0);
        assert!(
            items.iter().any(|i| i.label == "ex:"),
            "the declared `ex` prefix must be offered as a completion",
        );
    }

    #[test]
    fn well_known_prefix_carries_auto_import() {
        let db = db();
        let f = file(&db, "User : ex:Person from u\n");
        let items = completions(&db, &[f], f, 0, 0);
        let xsd = items
            .iter()
            .find(|i| i.label == "xsd:")
            .expect("the well-known xsd prefix must be offered");
        let edits = xsd
            .additional_text_edits
            .as_ref()
            .expect("a non-declared well-known prefix must carry an auto-import edit");
        assert!(
            edits[0].new_text.contains("prefix xsd:"),
            "well-known prefix auto-import must insert the prefix decl; got {:?}",
            edits[0].new_text,
        );
    }

    /// Source 4: with a host-registered `InferredDescriptor`, a `.` field
    /// reference offers the source's columns as Field completions.
    #[test]
    fn offers_source_fields_after_dot_when_descriptor_registered() {
        use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
        let db = db();
        db.system.register_inferred_descriptor(InferredDescriptor {
            source_name: "u".into(),
            columns: vec![
                InferredColumn {
                    name: "name".into(),
                    primitive: "String".into(),
                },
                InferredColumn {
                    name: "age".into(),
                    primitive: "Integer".into(),
                },
            ],
            content_hash: String::new(),
        });
        // `ex:` must be declared so the mapping lowers (lower_mapping resolves
        // the shape prefix); the `.name` field ref sits on line 2.
        let src =
            "prefix ex: <https://example.org/>\nUser : ex:Person from u\n    ex:name = .name\n";
        let f = file(&db, src);
        // Cursor right after the `.` on line 2 → token_at_position picks the DOT.
        let dot = u32::try_from(src.lines().nth(2).unwrap().find('.').unwrap()).unwrap();
        let items = completions(&db, &[f], f, 2, dot + 1);
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
