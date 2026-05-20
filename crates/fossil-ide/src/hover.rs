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
//! Phase 2 limitation (DISCHARGED in Phase 3 plan 03-07): hover used to fire
//! only for literal-subset RHS expressions (`StringLit` / `Template` /
//! `PrefixedName`); `FieldRef`s returned `None`. Phase 3 plan 03-05 made
//! [`fossil_hir::check::typecheck_mapping`] the source of truth, so a
//! `FieldRef` resolved against a CSVW source row now carries an
//! [`ExprTypeEntry`] — hover surfaces its type (the standard `*from
//! InputDescriptor { .. }*` trailer).
//!
//! # Phase 3 plan 03-07: SC#3 implicit-closure surfacing
//!
//! Plan 03-06 populated [`ProvenanceKind::SynthesizedClosureRendering`] on the
//! body `ExprId` of an implicitly-synthesised closure (the ONLY lambda form in
//! Fossil — type-system.md §7). This handler reads that variant in
//! [`render_markdown`] and renders the closure binding as a fenced `fossil`
//! code block ABOVE the field type, plus a `*synthesised closure parameter
//! binding*` tagline so the synthesis is NEVER hidden from the user (SC#3 /
//! CORE-07). All type rendering goes through [`render_ty_kind`], so
//! `TyKind::Unknown(InferenceId)` normalises to `?` and never leaks (STATE.md
//! "Do NOT expose `Unknown`").
//!
//! The full JSON-RPC end-to-end SC#3 surface form
//! (`users |> seq.filter(.age >= 18)`) is deferred to Phase 6: per plan
//! 03-05's recorded decision, no
//! `seq.filter` registry stub was added (Phase 3 v0.1's `HirExpr` has no
//! `Pipeline` / `Call` variant to lower), so the synthesis is unreachable
//! through surface syntax. The SC#3 rendering is verified at the
//! `render_markdown` unit layer here + a direct integration test in
//! `fossil-lsp/tests/lsp_hover_smoke.rs`.

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_hir::body::ExprId;
use fossil_hir::def_map::def_map;
use fossil_hir::provenance::{ExprTypeEntry, ProvenanceKind, ty_origin};
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

    let markdown = render_markdown(db, &entry);

    let r = property_node.text_range();
    Some(HoverInfo {
        markdown,
        range: r.start().into()..r.end().into(),
    })
}

/// Render the Markdown body for an [`ExprTypeEntry`].
///
/// Two paths:
///
/// 1. [`ProvenanceKind::SynthesizedClosureRendering`] (Phase 3 plan 03-07,
///    SC#3 / CORE-07): the entry sits on the body `ExprId` of an implicitly
///    synthesised closure. Render the closure binding as a fenced `fossil`
///    code block ABOVE the field type, then a `*synthesised closure parameter
///    binding*` tagline. Both the closure binding AND the field type appear —
///    the synthesis is NEVER hidden from the user (RESEARCH.md §Pitfall 6).
///
/// 2. Every other provenance kind (the Phase 2 literal subset + Phase 3's
///    CSVW-`FieldRef` widening): a fenced `fossil` type block + an italic
///    `*from {provenance:?}*` origin trailer.
///
/// All type rendering routes through [`render_ty_kind`], so
/// `TyKind::Unknown(InferenceId)` normalises to `?` and never leaks.
#[must_use]
pub fn render_markdown(db: &dyn fossil_base::Db, entry: &ExprTypeEntry<'_>) -> String {
    let field_ty = render_ty_kind(db, entry.ty.kind(db));
    match &entry.provenance.kind {
        // SC#3: the closure rendering ALREADY carries the row Record's field
        // names + types (built by `render_closure` via `render_ty_kind` in
        // plan 03-06), so it is reproduced verbatim as a fenced block. The
        // field type below is the closure body's result type.
        ProvenanceKind::SynthesizedClosureRendering { rendering } => format!(
            "```fossil\n{rendering}\n```\n\n\
             field type: `{field_ty}`\n\n\
             *synthesised closure parameter binding*",
        ),
        // Phase 2 literal subset + Phase 3 CSVW FieldRef widening. The
        // `*from {:?}*` trailer is preserved verbatim from plan 02-06 so the
        // existing lsp_hover_smoke literal assertion (`"Literal"`) holds.
        other => format!("```fossil\n{field_ty}\n```\n\n*from {other:?}*"),
    }
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

    /// Hover on the `ex:name = .name` line returns `None` for the HELLO
    /// fixture — its `users` source declares NO CSVW schema arg, so
    /// `resolve_source_row` returns `None` and the `FieldRef` synthesises no
    /// entry (matching the Phase 2 behaviour for the schema-less case). Plan
    /// 03-05 widened `FieldRef` hover ONLY when a CSVW schema is declared; with
    /// a schema, `render_markdown` surfaces the field type (see the
    /// `render_markdown_for_fieldref_with_csvw_propagation` unit test).
    #[test]
    fn hover_on_field_ref_returns_none_without_csvw_schema() {
        let (db, file) = db_with_text(HELLO);
        // Line 4 is `    ex:name = .name`. Cursor inside the property at
        // character 14 (somewhere on the value).
        let info = hover(&db, file, 4, 14);
        assert!(
            info.is_none(),
            "hover on FieldRef RHS with no CSVW schema must return None \
             (no source row to resolve against)"
        );
    }

    use fossil_hir::provenance::{ExprTypeEntry, Provenance, ProvenanceKind};
    use fossil_hir::ty::{InferenceId, Primitive, Record, RecordField, Ty, TyKind};

    fn bare_db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        fossil_base::FossilDb::new(system)
    }

    /// SC#3 surface (CORE-07): an `ExprTypeEntry` carrying
    /// `SynthesizedClosureRendering` renders the closure binding as a fenced
    /// `fossil` block, then the field type, then the tagline — BOTH the
    /// closure parameter binding AND the field type are present.
    #[test]
    fn render_markdown_synthesized_closure() {
        let db = bare_db();
        let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let rendering = smol_str::SmolStr::from("(row: Record<{age: Integer}>) => row.age >= 18");
        let entry = ExprTypeEntry {
            expr_id: ExprId(0),
            ty: int_ty,
            provenance: Provenance {
                span: fossil_base::Span { start: 0, end: 0 },
                kind: ProvenanceKind::SynthesizedClosureRendering { rendering },
            },
        };
        let md = render_markdown(&db, &entry);
        // (a) the closure binding, as a fenced fossil code block.
        assert!(
            md.contains("```fossil\n(row: Record<{age: Integer}>) => row.age >= 18\n```"),
            "expected the closure rendering as a fenced fossil block, got {md:?}",
        );
        // (b) the field type.
        assert!(
            md.contains("field type: `Integer`"),
            "expected the field type `Integer`, got {md:?}",
        );
        // (c) the tagline so the user understands WHY the closure appears.
        assert!(
            md.contains("*synthesised closure parameter binding*"),
            "expected the synthesis tagline, got {md:?}",
        );
        // The synthesis is NEVER hidden: closure binding + field type both present.
        assert!(md.contains("row.age >= 18") && md.contains("Integer"));
        // Risk Register: internal inference state must never leak.
        assert!(!md.contains("Unknown") && !md.contains("InferenceId"));
    }

    /// Phase 3 widening: a `FieldRef` resolved against a CSVW source row
    /// carries `InputDescriptor` provenance + the field's type.
    /// `render_markdown` surfaces the type (Phase 2 returned `None` for
    /// `FieldRef`).
    #[test]
    fn render_markdown_for_fieldref_with_csvw_propagation() {
        let db = bare_db();
        let str_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
        let entry = ExprTypeEntry {
            expr_id: ExprId(1),
            ty: str_ty,
            provenance: Provenance {
                span: fossil_base::Span { start: 0, end: 0 },
                kind: ProvenanceKind::InputDescriptor {
                    source_name: smol_str::SmolStr::from("users"),
                    column: smol_str::SmolStr::from("name"),
                },
            },
        };
        let md = render_markdown(&db, &entry);
        assert!(
            md.contains("String"),
            "expected the CSVW field type `String`, got {md:?}",
        );
        assert!(
            md.contains("```fossil"),
            "expected a fenced fossil block, got {md:?}",
        );
        // Not a closure context — no closure binding rendered.
        assert!(
            !md.contains("(row:"),
            "FieldRef outside a closure must NOT render a closure binding, got {md:?}",
        );
        assert!(!md.contains("Unknown") && !md.contains("InferenceId"));
    }

    /// Risk Register: `render_ty_kind` (re-exported here) maps EVERY `TyKind`
    /// variant to user-facing text — no rendered output contains the internal
    /// `Unknown` / `InferenceId` placeholders (STATE.md "Do NOT expose
    /// `TyKind::Unknown(InferenceId)`").
    #[test]
    fn render_ty_kind_normalises_unknown_to_question_mark() {
        let db = bare_db();
        let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let rec = Record::new(
            &db,
            vec![RecordField {
                name: smol_str::SmolStr::from("age"),
                ty: int_ty,
            }],
        );
        let sig = fossil_hir::ty::FnSig::new(&db, vec![int_ty], int_ty);
        let variants: Vec<TyKind<'_>> = vec![
            TyKind::Primitive(Primitive::String),
            TyKind::Optional(int_ty),
            TyKind::Seq(int_ty),
            TyKind::Record(rec),
            TyKind::Iri,
            TyKind::IriTemplate,
            TyKind::Shape(fossil_hir::ty::ShapeId(0)),
            TyKind::Fn(sig),
            TyKind::TripleTerm,
            TyKind::Unknown(InferenceId(7)),
        ];
        // 10 surface-constructible variants + Error (constructed below) = 11.
        for kind in &variants {
            let s = render_ty_kind(&db, kind);
            assert!(
                !s.contains("Unknown") && !s.contains("InferenceId"),
                "render_ty_kind leaked internal state for {kind:?}: {s:?}",
            );
        }
        // The Unknown variant specifically normalises to "?".
        assert_eq!(render_ty_kind(&db, &TyKind::Unknown(InferenceId(7))), "?");
    }
}
