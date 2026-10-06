//! `textDocument/hover` query — position → enclosing `PROPERTY` → `ty_origin`.
//!
//! The minimum delta that wires LSP hover end-to-end. The handler:
//!
//! 1. Resolves the LSP position to a [`fossil_syntax::SyntaxNode`] via
//!    [`crate::position::node_at_position`].
//! 2. Walks up to the enclosing `PROPERTY` node (the type-bearing position
//!    in the property grammar) AND the enclosing `MAPPING` node.
//! 3. Computes the per-MAPPING-kind dense index of the enclosing mapping —
//!    the position among MAPPING-kind top-level children, which is what
//!    `MappingLoc::index` and `body()`'s filter-then-nth contract mean.
//! 4. Computes the property's index inside `MAPPING_BODY` — this is the
//!    `ExprId`, one per lowered property in source order.
//! 5. Calls [`fossil_hir::provenance::ty_origin`] for the
//!    `(MappingLoc, ExprId)` pair. Returns `None` when the checker inferred
//!    no type for the RHS expression.
//! 6. Destructures the returned [`ExprTypeEntry`] — a named struct, not a
//!    tuple — into `ty` + `provenance`.
//! 7. Renders the type kind as Markdown: a fenced code block ```` ```fossil ````
//!    + the rendered `TyKind` + an italic provenance trailer.
//!
//! [`fossil_hir::check::typecheck_mapping`] is the source of truth, so a
//! `FieldRef` resolved against a source row carries an [`ExprTypeEntry`] and
//! hover surfaces its type with a `*from the column `User.age`*` trailer.
//! All type rendering goes through [`render_ty_kind`], never `{:?}`, so no
//! `TyKind` reaches a hover under its Rust spelling.

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_hir::body::{ExprId, body};
use fossil_hir::check::typecheck_mapping;
use fossil_hir::lower::PropertyKey;
use fossil_hir::provenance::{ExprTypeEntry, ty_origin};
use fossil_hir::shapes::resolve_target_shape;
use fossil_syntax::SyntaxKind;

// `render_ty_kind` lives in `fossil_hir::ty::display`, the single source of
// truth for type pretty-printing. It is re-exported here so a caller reaching
// for `fossil_ide::hover::render_ty_kind` gets that one and this module needs no
// local definition to drift from it.
pub use fossil_hir::render_ty_kind;

use crate::position::node_at_position;

/// Hover result: Markdown body + the source range the hover applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    /// Markdown content (`Hover.contents` per LSP spec): a ```` ```fossil ````
    /// fence + the rendered type kind + an italic provenance line.
    pub markdown: String,
    /// Byte-offset range the hover applies to. Translated to
    /// `lsp_types::Range` by the LSP handler in `fossil-lsp/src/lib.rs`.
    pub range: Range<u32>,
}

/// Compute hover info at an LSP position: the source-side type, and the
/// target-side one when the mapping's `ShEx` shape resolves.
///
/// `None` if no type-bearing expression is at the cursor (whitespace, an
/// expression the checker inferred no type for, a position outside any
/// MAPPING).
///
/// The target shape comes from [`resolve_target_shape`], which reads the
/// document the PROGRAM names.
/// When the hovered `.field`'s predicate matches a shape
/// constraint carrying a value type, the rendered Markdown appends a SECOND
/// fenced block showing the **target-side** type (from
/// `ShapeConstraint::value_ty`).
///
/// The "if reachable" hedge: if no shape resolves (the program
/// names no document, the host has not registered the one it names — see
/// [`fossil_hir::documents`] — the document omits the mapping's shape, or the
/// predicate has no constraint), the hover shows the source-side block only —
/// best-effort, no error. The binding reports the failures; a hover is not the
/// place to.
///
/// All type rendering routes through [`render_ty_kind`], so `TyKind::Unknown`
/// never leaks: internal inference state must not appear in a hover.
#[must_use]
pub fn hover(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<HoverInfo> {
    let resolved = resolve_hover_target(db, file, line, character)?;
    let entry: ExprTypeEntry<'_> = ty_origin(db, resolved.mapping, resolved.expr_id)?;

    // Target-side: resolve the mapping's ShEx shape against the host descriptor
    // and find the constraint matching the hovered property's predicate IRI.
    let target_block = resolved.predicate_iri.as_deref().and_then(|pred| {
        let shape = resolve_target_shape(db, resolved.mapping)?;
        let constraint = shape.constraint_for(pred)?;
        // `value_ty == None` means "any value" (no datatype narrowing) — render
        // it as `Iri` (the constraint's default node type), consistent with the
        // checker's `unwrap_or_else(|| Iri)` in `check_property`.
        let ty_str = constraint
            .value_ty
            .map_or_else(|| "Iri".to_string(), |ty| render_ty_kind(db, ty.kind(db)));
        Some(ty_str)
    });

    let markdown = render_markdown(db, &entry, target_block.as_deref());
    Some(HoverInfo {
        markdown,
        range: resolved.range,
    })
}

/// The position-resolution result [`hover`] reads.
struct ResolvedHover<'db> {
    mapping: fossil_hir::def_map::MappingLoc<'db>,
    expr_id: ExprId,
    /// The hovered property's fully-resolved predicate IRI, if it is a
    /// `prefix:local` predicate (not the subject `iri =` property).
    predicate_iri: Option<String>,
    range: Range<u32>,
}

/// Resolve an LSP position to the enclosing mapping + property `ExprId` +
/// predicate IRI + source range.
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Salsa-handle lifetime contract
fn resolve_hover_target<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<ResolvedHover<'db>> {
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

    let mapping = fossil_hir::def_map::mapping_of(db, file, &mapping_node)?;

    // The `ExprId` under the cursor, BY SPAN CONTAINMENT.
    //
    // This counted instead: the property's position among the CST's `PROPERTY`
    // children, used both as the `ExprId` and (below) as an index into the HIR's
    // `properties` vector. Those are two different numbers. `body()` numbers
    // densely over the properties that LOWERED, and a property that parses and
    // does not lower — every retired spelling in the corpus is one: a CURIE key,
    // `iri =`, an absolute-IRI key — makes the CST longer than the HIR and
    // shifts everything behind it. The hover then showed the short name and the
    // predicate IRI of a DIFFERENT property, with no way to tell.
    //
    // `body()` publishes one span per lowered property at the property's own
    // index, so the id is a lookup and the two vectors cannot drift: they are
    // the same vector's indices.
    let hir_body = body(db, mapping);
    // `expr_spans` are mapping-relative (rowan resets offsets at the detached
    // per-mapping root); the CST node here is in the file tree. Rebase the
    // PROPERTY's range down, then take the property whose RHS span sits inside
    // it — the RHS of a property is inside exactly one property.
    let base = fossil_hir::spans::mapping_start_offset(db, mapping);
    let range = property_node.text_range();
    let lo = u32::from(range.start()).saturating_sub(base);
    let hi = u32::from(range.end()).saturating_sub(base);
    let property_index = hir_body
        .expr_spans(db)
        .iter()
        .position(|s| s.start >= lo && s.end <= hi)?;
    let expr_id = ExprId(u32::try_from(property_index).unwrap_or(u32::MAX));

    // The hovered property's predicate IRI (for matching a shape constraint).
    //
    // The key no longer carries it: a body writes a bare `name`, and the IRI is
    // the shape document's to know. So the short name comes from
    // the SAME lowered `HirProperty` the checker uses, and the IRI comes from
    // the checker's own `predicates` table — the one `fossil-mir` reads for
    // `rdf_uri`. One table, two consumers, no second resolution.
    //
    // `PropertyKey::Subject` is not a predicate at all: a shape declares a
    // node's predicates, and in RDF the subject IS the node.
    let short_name = hir_body
        .properties(db)
        .get(property_index)
        .and_then(|prop| match &prop.key {
            PropertyKey::Name(name) => Some(name.clone()),
            PropertyKey::Subject => None,
        });
    let predicate_iri = short_name.and_then(|name| {
        let output = typecheck_mapping(db, mapping).ok()?;
        output
            .predicates(db)
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.iri.to_string())
    });

    let r = property_node.text_range();
    Some(ResolvedHover {
        mapping,
        expr_id,
        predicate_iri,
        range: r.start().into()..r.end().into(),
    })
}

/// Render the Markdown body for an [`ExprTypeEntry`], optionally appending a
/// **target-side** type block.
///
/// The source side is a fenced `fossil` type block and an italic
/// `*from <provenance>*` trailer, the provenance in the program's words.
/// When `target_ty` is `Some(rendered)`, a SECOND fenced `fossil` block plus a
/// `*target type (ShEx shape constraint)*` tagline is appended, so the user
/// sees BOTH the source-side type (from the input descriptor provenance) AND the
/// target-side type (from the resolved `ShEx` shape). When `target_ty` is
/// `None` (no shape resolved — no document named / unreachable, the "if reachable"
/// hedge), only the source-side block is rendered — no error.
///
/// `target_ty` is always pre-rendered through [`render_ty_kind`] by the caller,
/// so `TyKind::Unknown` never leaks here either.
#[must_use]
pub fn render_markdown(
    db: &dyn fossil_base::Db,
    entry: &ExprTypeEntry<'_>,
    target_ty: Option<&str>,
) -> String {
    let field_ty = render_ty_kind(db, entry.ty.kind(db));
    let source_side = format!(
        "```fossil\n{field_ty}\n```\n\n*from {}*",
        entry.provenance.kind
    );

    // Append the target-side (ShEx) type block when a shape resolved.
    match target_ty {
        Some(target) => format!(
            "{source_side}\n\n\
             ```fossil\n{target}\n```\n\n\
             *target type (ShEx shape constraint)*",
        ),
        None => source_side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO: &str = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"x.csv\")
User : Person from users
    @subject = \"https://example.org/u/{users.id}\"
    name = users.name
";

    fn db_with_text(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "hover.fossil".to_string());
        (db, file)
    }

    /// Hover on the identity line (line 3) returns Markdown naming the type —
    /// a REFERENCE — and the `Literal` provenance.
    ///
    /// It read `IriTemplate`, and that type is gone: an identity is a reference
    /// to the shape the mapping produces. This fixture registers no document,
    /// so the shape resolves to nothing and the rendering is `Ref<?>`.
    #[test]
    fn hover_on_the_identity_names_a_reference() {
        let (db, file) = db_with_text(HELLO);
        // Lines in HELLO are 0-indexed:
        //   0: type { Person } := io.shex(...)
        //   1: users := io.csv(...)
        //   2: User : Person from users
        //   3:     @subject = "https://example.org/u/{users.id}"
        //   4:     name = users.name
        // Cursor inside the interpolated string at character 20, comfortably
        // inside the property.
        let info = hover(&db, file, 3, 20).expect("hover at (3, 20) must return Some");
        assert!(
            info.markdown.contains("Ref<"),
            "expected hover markdown to name a reference, got {:?}",
            info.markdown,
        );
        assert!(
            info.markdown.contains("*from a literal*"),
            "expected hover markdown to mention Literal provenance, got {:?}",
            info.markdown,
        );
        assert!(
            info.markdown.contains("```fossil"),
            "expected hover markdown to contain a fenced fossil code block, got {:?}",
            info.markdown,
        );
    }

    /// Hover on the `name = users.name` line returns `None` for the HELLO
    /// fixture — no descriptor is registered for its `users` source, so
    /// `resolve_source_scope` returns `None` and the `FieldRef` synthesises no
    /// entry. A `FieldRef` hover needs a row to resolve against; given one,
    /// `render_markdown` surfaces the field type (see the
    /// `render_markdown_for_fieldref_with_forward_propagation` unit test).
    #[test]
    fn hover_on_field_ref_returns_none_without_a_source_row() {
        let (db, file) = db_with_text(HELLO);
        // Line 4 is `    name = users.name`. Cursor inside the property at
        // character 14 (somewhere on the value).
        let info = hover(&db, file, 4, 14);
        assert!(
            info.is_none(),
            "hover on FieldRef RHS with no registered descriptor must return \
             None (no source row to resolve against)"
        );
    }

    use fossil_graph_schema::Primitive;
    use fossil_hir::provenance::{ExprTypeEntry, Provenance, ProvenanceKind};
    use fossil_hir::ty::{Record, RecordField, Ty, TyKind};

    fn bare_db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    /// A `FieldRef` resolved against a source row carries `InputDescriptor`
    /// provenance + the field's type, and `render_markdown` surfaces that type.
    #[test]
    fn render_markdown_for_fieldref_with_forward_propagation() {
        let db = bare_db();
        let str_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
        let entry = ExprTypeEntry {
            expr_id: ExprId(1),
            ty: str_ty,
            provenance: Provenance {
                span: fossil_graph_schema::Span { start: 0, end: 0 },
                kind: ProvenanceKind::InputDescriptor {
                    source_name: smol_str::SmolStr::from("users"),
                    column: smol_str::SmolStr::from("name"),
                },
            },
        };
        let md = render_markdown(&db, &entry, None);
        assert!(
            md.contains("String"),
            "expected the field type `String`, got {md:?}",
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

    /// `render_markdown` with a `Some(target)` appends a SECOND fenced block +
    /// the `ShEx` tagline — BOTH the source-side type and the target-side type
    /// appear, in that order. With `None` there is no target block (the "if
    /// reachable" hedge).
    #[test]
    fn render_markdown_appends_a_target_block() {
        let db = bare_db();
        let str_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
        let entry = ExprTypeEntry {
            expr_id: ExprId(1),
            ty: str_ty,
            provenance: Provenance {
                span: fossil_graph_schema::Span { start: 0, end: 0 },
                kind: ProvenanceKind::InputDescriptor {
                    source_name: smol_str::SmolStr::from("users"),
                    column: smol_str::SmolStr::from("name"),
                },
            },
        };

        // Target-side resolved (ShEx demands `Integer` for this predicate).
        let md = render_markdown(&db, &entry, Some("Integer"));
        // Source-side block (`String`, from the input descriptor) present.
        assert!(
            md.contains("String"),
            "source-side type `String` must still appear; got {md:?}",
        );
        // Target-side block (ShEx Integer) present.
        assert!(
            md.contains("Integer"),
            "target-side type `Integer` must appear; got {md:?}",
        );
        // The ShEx tagline explains the second block.
        assert!(
            md.contains("target type (ShEx shape constraint)"),
            "the target-side block must carry the ShEx tagline; got {md:?}",
        );
        // Source block comes BEFORE the target block.
        let src_pos = md.find("String").unwrap();
        let tgt_pos = md.find("target type (ShEx shape constraint)").unwrap();
        assert!(
            src_pos < tgt_pos,
            "source-side block must precede the target-side block; got {md:?}",
        );
        // Two fenced fossil blocks (source + target).
        assert_eq!(
            md.matches("```fossil").count(),
            2,
            "expected exactly two fenced fossil blocks; got {md:?}",
        );

        // `None` → the source-side block alone.
        let md_none = render_markdown(&db, &entry, None);
        assert_eq!(md_none.matches("```fossil").count(), 1);
        assert!(!md_none.contains("target type (ShEx shape constraint)"));
        assert!(!md_none.contains("Unknown") && !md_none.contains("InferenceId"));
    }

    /// `render_ty_kind` (re-exported here) maps EVERY `TyKind` variant to
    /// user-facing text — no rendered output contains the internal
    /// `Unknown` / `InferenceId` placeholders, because
    /// `TyKind::Unknown(InferenceId)` is inference state and must never appear
    /// in a surface diagnostic or a hover.
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
        let variants: Vec<TyKind<'_>> = vec![
            TyKind::Primitive(Primitive::String),
            TyKind::Seq(int_ty),
            TyKind::Record(rec),
            TyKind::Ref(vec![smol_str::SmolStr::new_static(
                "https://example.org/Person",
            )]),
        ];
        // Every variant a caller can construct. There is no count to quote and
        // there used to be («10 surface-constructible + Error = 11»): three of
        // the eleven — `Optional`, `Fn` and `Unknown` — were built by this list
        // and by nothing else in the workspace, and all three are gone. The
        // third is why this assertion is still worth making: it was CHECKER
        // state given a type, and this test existed to keep it off the screen.
        // A kind that cannot be constructed cannot leak.
        for kind in &variants {
            let s = render_ty_kind(&db, kind);
            assert!(
                !s.contains("Unknown") && !s.contains("InferenceId"),
                "render_ty_kind leaked internal state for {kind:?}: {s:?}",
            );
        }
    }
}
