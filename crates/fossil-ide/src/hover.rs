//! `textDocument/hover` query — position → enclosing `PROPERTY` → `ty_origin`.
//!
//! Phase 2 plan 02-06 (per RESEARCH.md §Q9): the minimum delta to wire the
//! LSP hover end-to-end. The handler:
//!
//! 1. Resolves the LSP position to a [`fossil_syntax::SyntaxNode`] via
//!    [`crate::position::node_at_position`].
//! 2. Walks up to the enclosing `PROPERTY` node (the type-bearing position
//!    in the property grammar) AND the enclosing `MAPPING` node.
//! 3. Computes the per-MAPPING-kind dense index of the enclosing mapping
//!    (matches `MappingLoc::index` and `body()`'s filter-then-nth contract
//!    per ADR-0005 / plan 02-04 Blocker 2).
//! 4. Computes the property's index inside `MAPPING_BODY` — this is the
//!    `ExprId` per plan 02-04's `expr_count` advancement (one `ExprId` per
//!    lowered property in source order).
//! 5. Calls [`fossil_hir::provenance::ty_origin`] for the
//!    `(MappingLoc, ExprId)` pair. Returns `None` if Phase 2's literal
//!    subset didn't infer a type for the RHS expression (e.g. `FieldRef`).
//! 6. Destructures the returned [`ExprTypeEntry`] (per planner checker
//!    Blocker 5 — NOT a tuple) into `ty` + `provenance`.
//! 7. Renders the type kind as Markdown: a fenced code block ```` ```fossil ````
//!    + the rendered `TyKind` + an italic provenance trailer.
//!
//! Phase 2 limitation: hover only fires for literal-subset RHS expressions
//! (`StringLit` / `Template` / `PrefixedName`). `FieldRef`s return `None`
//! (the source-row type comes from CSVW, Phase 3 territory).

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_hir::body::ExprId;
use fossil_hir::def_map::def_map;
use fossil_hir::provenance::{ExprTypeEntry, ty_origin};
use fossil_syntax::SyntaxKind;

// Phase 3 plan 03-05 (Serious #7): `render_ty_kind` was PROMOTED to
// `fossil_hir::ty::display` as the single source of truth for type
// pretty-printing. It is re-exported here for back-compat so existing
// `fossil_ide::hover::render_ty_kind` callers (and plan 03-07's hover
// widening) keep working without a local definition.
pub use fossil_hir::render_ty_kind;

use crate::position::node_at_position;

/// Hover result: Markdown body + the source range the hover applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    /// Markdown content (`Hover.contents` per LSP spec). Phase 2 fenced as
    /// ```` ```fossil ```` + rendered type kind + italic provenance line.
    pub markdown: String,
    /// Byte-offset range the hover applies to. Translated to
    /// `lsp_types::Range` by the LSP handler in `fossil-lsp/src/main.rs`.
    pub range: Range<u32>,
}

/// Compute hover info at an LSP position.
///
/// Returns `None` if no type-bearing expression is at the cursor (e.g.
/// cursor on whitespace, on a `FieldRef` whose type Phase 2 cannot
/// synthesise, or on a position outside any MAPPING).
#[must_use]
pub fn hover(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<HoverInfo> {
    let node = node_at_position(db, file, line, character)?;

    // Walk up to find the enclosing PROPERTY + MAPPING. PROPERTY may not
    // exist (cursor in MAPPING_HEADER); in that case we have no expression
    // to hover.
    let mut current = Some(node);
    let mut enclosing_property = None;
    let mut enclosing_mapping = None;
    while let Some(n) = current {
        if n.kind() == SyntaxKind::PROPERTY && enclosing_property.is_none() {
            enclosing_property = Some(n.clone());
        }
        if n.kind() == SyntaxKind::MAPPING {
            enclosing_mapping = Some(n);
            break;
        }
        current = n.parent();
    }
    let mapping_node = enclosing_mapping?;
    let property_node = enclosing_property?;

    let cst = fossil_syntax::parse(db, file);
    // Per ADR-0005 / plan 02-04 Blocker 2: MappingLoc.index is the position
    // among MAPPING-kind top-level children — filter BEFORE indexing.
    let mapping_index = cst
        .root(db)
        .syntax()
        .children()
        .filter(|c| c.kind() == SyntaxKind::MAPPING)
        .position(|c| c == mapping_node)?;
    let dm = def_map(db, file);
    let mapping = dm
        .mappings(db)
        .iter()
        .find(|m| m.index(db) == mapping_index)
        .copied()?;

    // ExprId is the property's dense index within MAPPING_BODY (matches
    // `body()` / `expr_types()` enumeration in plans 02-04 + 02-06).
    let body_node = mapping_node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)?;
    let property_index = body_node
        .children()
        .filter(|c| c.kind() == SyntaxKind::PROPERTY)
        .position(|p| p == property_node)?;
    let expr_id = ExprId(u32::try_from(property_index).unwrap_or(u32::MAX));

    // Per checker Blocker 5: ty_origin returns Option<ExprTypeEntry<'db>>
    // (NOT Option<(Ty, Provenance)>). We destructure the struct fields.
    let entry: ExprTypeEntry<'_> = ty_origin(db, mapping, expr_id)?;
    let ty = entry.ty;
    let prov = entry.provenance;

    let kind_str = render_ty_kind(db, ty.kind(db));
    let markdown = format!("```fossil\n{kind_str}\n```\n\n*from {:?}*", prov.kind);

    let r = property_node.text_range();
    Some(HoverInfo {
        markdown,
        range: r.start().into()..r.end().into(),
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

    fn db_with_text(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "hover.fossil".to_string());
        (db, file)
    }

    /// Hover on the iri template property line (line 3) returns Markdown
    /// containing `IriTemplate` (the rendered ty kind) AND `Literal` (the
    /// provenance kind).
    #[test]
    fn hover_on_iri_template_returns_iri_template_markdown() {
        let (db, file) = db_with_text(HELLO);
        // Lines in HELLO are 0-indexed:
        //   0: prefix ex: <...>
        //   1: users := io.csv(...)
        //   2: User : ex:Person from users
        //   3:     iri = `${ex:}u/${.id}`
        //   4:     ex:name = .name
        // Cursor inside the template text at character 10 (within the iri
        // line, comfortably inside the property).
        let info = hover(&db, file, 3, 10).expect("hover at (3, 10) must return Some");
        assert!(
            info.markdown.contains("IriTemplate"),
            "expected hover markdown to mention IriTemplate, got {:?}",
            info.markdown,
        );
        assert!(
            info.markdown.contains("Literal"),
            "expected hover markdown to mention Literal provenance, got {:?}",
            info.markdown,
        );
        assert!(
            info.markdown.contains("```fossil"),
            "expected hover markdown to contain a fenced fossil code block, got {:?}",
            info.markdown,
        );
    }

    /// Hover on the `ex:name = .name` line returns `None` — the RHS is a
    /// `FieldRef`, deferred to Phase 3.
    #[test]
    fn hover_on_field_ref_returns_none_phase_2() {
        let (db, file) = db_with_text(HELLO);
        // Line 4 is `    ex:name = .name`. Cursor inside the property at
        // character 10 (somewhere on the value).
        let info = hover(&db, file, 4, 14);
        assert!(
            info.is_none(),
            "Phase 2 hover on FieldRef RHS must return None (deferred to Phase 3)"
        );
    }
}
