//! Per-mapping real-span side table — Phase 3 plan 03-04 (ADR-0008).
//!
//! Replaces Phase 2's zero-width `Span { start: 0, end: 0 }` placeholders for
//! literal-subset provenance entries (see plan 02-06) with real byte ranges
//! read from `rowan::TextRange` during lowering.
//!
//! # Why a side table (not a field on `HirExpr`)
//!
//! Same architectural rationale as the Phase 2 provenance side table
//! (`crate::provenance`): adding `span` to every `HirExpr` variant would
//! change its `PartialEq`/`Hash`, which Salsa uses for `salsa::Update`. Two
//! source-identical expressions at different source positions would no
//! longer dedupe in interning, defeating the "structural equality at the
//! HIR layer → pointer equality after Salsa interning" contract per
//! Phase 3 RESEARCH.md §Pattern 2.
//!
//! Three options were considered (see ADR-0008):
//! 1. Add `span: Span` field to every `HirExpr` variant. REJECTED — breaks
//!    Salsa interning.
//! 2. Per-mapping `Spans<'db>` side table keyed by `(MappingLoc, ExprId)`,
//!    populated during lowering. CHOSEN.
//! 3. On-demand span lookup at diagnostic emission time via CST walk.
//!    REJECTED — wasteful (re-walks the CST every time a diagnostic is
//!    emitted) and the lowering arena is the right place to record
//!    positions since it's already navigating the CST node-by-node.
//!
//! # Salsa invalidation barrier
//!
//! The [`spans`] tracked query reads [`mapping_cst_node(db, mapping)`] —
//! the SAME source `crate::body::body` reads. This is REQUIRED so the
//! Phase 2 plan 02-07 `MAX_PER_MAPPING_FAN_OUT = 1` invariant continues to
//! hold. Reading `parse(db, file)` directly would tie every per-mapping
//! spans query to the whole-file CST, causing all sibling spans to
//! re-execute on any body edit. The intermediate per-mapping CST barrier
//! from ADR-0005 + plan 02-07 prevents this — see
//! `crates/fossil-hir/src/body.rs`'s `mapping_cst_node` documentation.
//!
//! # `ExprId` convention
//!
//! Phase 2 establishes `ExprId(i) = i'th property's RHS expression`. Phase 3
//! plan 03-04 retains this convention (`HirExpr` in Phase 2 is non-recursive —
//! `Template`/`FieldRef`/`StringLit`/`PrefixedName` are all leaf forms). Per
//! the plan, every `HirExpr` therefore has an `ExprId`. Subexpression-level
//! arena allocation (for nested function calls, ternaries, etc.) lands when
//! the Pratt-lowered expression tree extends `HirExpr` in a later plan.
//!
//! # Offset semantics: mapping-relative
//!
//! The recorded [`Span`]s are RELATIVE to the start of the mapping (not
//! file-absolute). This is a direct consequence of reading
//! [`mapping_cst_node`] rather than `parse(file)`: rowan's
//! `SyntaxNode::new_root(green)` resets offsets to zero, so
//! `text_range()` values inside a per-mapping subtree are mapping-relative.
//!
//! This is the right trade-off — file-absolute offsets would require
//! reading `parse(db, file)` directly, defeating the per-mapping
//! invalidation barrier. Mapping-relative offsets are stable across
//! sibling-mapping edits (only the mapping's own content moves spans;
//! shifts in OTHER mappings don't cause `Spans(M_k)` to re-execute).
//!
//! The diagnostic-emission layer (`crate::check::compatible` in plan
//! 03-05, the LSP host) is responsible for converting mapping-relative
//! offsets to file-absolute when needed. The conversion is a single
//! lookup of the mapping's start offset in the file CST and an add.
//!
//! # Lookup complexity
//!
//! [`Spans::get`] is a linear scan over `Vec<(ExprId, Span)>`. Typical
//! mapping has ≤50 expressions; for the LSP hover hot path the lookup is
//! one `expr_id` per request. If profiling later shows this hot, swap for
//! `HashMap<u32, Span>` or sorted-Vec + binary search — the public API
//! shape (`spans(db, m).get(db, expr_id)`) stays unchanged.

use fossil_base::Span;

use crate::body::{ExprId, mapping_cst_node};
use crate::def_map::MappingLoc;

/// Per-mapping real-span side table.
///
/// Indexed-by-position into `ExprId(u32)` → vector position. The
/// `crate::check::compatible` two-span blame in plan 03-05 reads this via
/// [`Spans::get`] for both source and destination blame positions.
#[salsa::tracked(debug)]
pub struct Spans<'db> {
    /// Per-expression spans. One entry per Phase 2 property RHS (matches
    /// the `expr_id = property_index` convention established in plan 02-06).
    #[returns(ref)]
    pub by_expr: Vec<(ExprId, Span)>,
}

impl<'db> Spans<'db> {
    /// Look up the real byte range for an [`ExprId`]. Returns `None` for
    /// unknown ids (defensive — Phase 3 callers should always have a valid
    /// id from `body(db, mapping).expr_count(db)`).
    #[must_use]
    pub fn get(self, db: &'db dyn fossil_base::Db, expr_id: ExprId) -> Option<Span> {
        self.by_expr(db)
            .iter()
            .find(|(id, _)| *id == expr_id)
            .map(|(_, s)| *s)
    }
}

/// Per-mapping spans query — populates [`Spans`] from real `rowan::TextRange`s.
///
/// Reads [`mapping_cst_node`] (NOT `parse(db, file)`) so the per-mapping
/// invalidation barrier from ADR-0005 + plan 02-07 covers spans too. Walks
/// the MAPPING's `MAPPING_BODY > PROPERTY > EXPR` subtree in lockstep with
/// `crate::body::body`'s lowering — one `ExprId` per `PROPERTY`'s RHS
/// `EXPR` node, with its full `text_range()` as the recorded span.
///
/// The `EXPR` composite node wraps the RHS of every property (per Phase 2
/// plan 02-03 parser; see `crate::lower::lower_expr`). Its `text_range()`
/// covers the entire right-hand side — e.g., for `iri = \`${ex:}u/${.id}\``
/// the span covers the backtick-delimited template; for `ex:name = .name`
/// the span covers `.name`.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn spans<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> Spans<'db> {
    use fossil_syntax::SyntaxKind;

    let mapping_cst = mapping_cst_node(db, mapping);
    let mut by_expr: Vec<(ExprId, Span)> = Vec::new();

    if let Some(node) = mapping_cst.syntax()
        && let Some(body_node) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)
    {
        for (prop_index, prop_node) in body_node
            .children()
            .filter(|c| c.kind() == SyntaxKind::PROPERTY)
            .enumerate()
        {
            // Skip properties that wouldn't have lowered to an ExprId — the
            // body() query short-circuits via `?` if the LHS or RHS doesn't
            // resolve. Spans matches body()'s indexing so the ExprId
            // assignment stays consistent.
            //
            // For the Phase 2 literal subset, descend EXPR → its first
            // non-trivia child (TEMPLATE_EXPR, FIELD_REF_EXPR, LITERAL_EXPR,
            // or IRI_EXPR per `crate::lower::lower_expr`). The EXPR composite
            // node's own `text_range()` includes leading/trailing trivia
            // (whitespace + newlines); the inner-kind child gives a tight
            // span covering exactly the RHS expression tokens.
            if let Some(expr_node) = prop_node.children().find(|c| c.kind() == SyntaxKind::EXPR)
                && let Some(inner) = expr_node.children().next()
            {
                let range = inner.text_range();
                let span = Span {
                    start: u32::from(range.start()),
                    end: u32::from(range.end()),
                };
                let expr_id = ExprId(u32::try_from(prop_index).unwrap_or(u32::MAX));
                by_expr.push((expr_id, span));
            }
        }
    }

    Spans::new(db, by_expr)
}

/// Byte offset of a mapping's CST node within its file.
///
/// The counterpart to this module's mapping-relative offsets: add this to any
/// recorded [`Span`] to get a file-absolute one.
///
/// DELIBERATELY NOT a `#[salsa::tracked]` query. It reads `parse(db, file)`,
/// which is exactly the whole-file read the per-mapping barrier exists to keep
/// out of the compile path (ADR-0005). Callers are diagnostic-EMISSION layers,
/// which sit outside that barrier and already hold the file text.
#[must_use]
pub fn mapping_start_offset<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> u32 {
    fossil_syntax::parse(db, mapping.file(db))
        .root(db)
        .syntax()
        .children()
        .filter(|n| n.kind() == fossil_syntax::SyntaxKind::MAPPING)
        .nth(mapping.index(db))
        .map_or(0, |n| u32::from(n.text_range().start()))
}

/// Rebase a mapping's diagnostics from mapping-relative onto file-absolute
/// offsets, so a host can render them against the file text.
///
/// EVERY diagnostic-emission layer must call this. Skipping it does not fail
/// loudly — it silently points the squiggle at whatever happens to sit at that
/// offset from the start of the FILE, which for any mapping but the first is
/// another mapping entirely. It rebases [`Diagnostic::did_you_mean`]'s
/// `wrong_span` too: that one drives a quick-fix `WorkspaceEdit`, so a stale
/// offset there does not just mislead, it edits the wrong range.
#[must_use]
pub fn rebase_to_file<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    diagnostics: impl IntoIterator<Item = fossil_base::Diagnostic>,
) -> Vec<fossil_base::Diagnostic> {
    let base = mapping_start_offset(db, mapping);
    let shift = |s: Span| Span {
        start: s.start.saturating_add(base),
        end: s.end.saturating_add(base),
    };
    diagnostics
        .into_iter()
        .map(|mut d| {
            d.span = shift(d.span);
            if let Some(dym) = d.did_you_mean.as_mut() {
                dym.wrong_span = shift(dym.wrong_span);
            }
            d
        })
        .collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::def_map::def_map;
    use std::sync::Arc;

    fn db_with_text(src: &str, name: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), name.to_string());
        (db, file)
    }

    /// A rebased span must select the SAME text from the file that the raw
    /// span selects from the mapping. Regression guard: before
    /// [`rebase_to_file`], every emission layer rendered mapping-relative
    /// offsets against the file, so a diagnostic in any mapping but the first
    /// pointed at an unrelated earlier one.
    #[test]
    fn rebase_lands_on_the_same_text_in_the_file() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
First : ex:A from users
    iri = `${ex:}a/${.id}`
    ex:name = .name

Second : ex:B from users
    iri = `${ex:}b/${.id}`
    ex:name = .other
";
        let (db, file) = db_with_text(SRC, "two.fossil");
        let second = *def_map(&db, file)
            .mappings(&db)
            .get(1)
            .expect("Second is the 2nd mapping");

        let raw = spans(&db, second)
            .get(&db, ExprId(1))
            .expect("the 2nd property's RHS");
        let local = &mapping_text(&db, second)[raw.start as usize..raw.end as usize];
        assert_eq!(local, ".other", "sanity: the raw span is mapping-relative");

        let base = mapping_start_offset(&db, second);
        assert!(base > 0, "the 2nd mapping does not start at the file head");
        let absolute = &SRC[(raw.start + base) as usize..(raw.end + base) as usize];
        assert_eq!(absolute, local, "rebased span selects the same text in the file");

        // And the whole-diagnostic path shifts `did_you_mean.wrong_span` too —
        // that one drives a quick-fix edit, so a stale offset corrupts source.
        let d = fossil_base::Diagnostic::new(fossil_base::Severity::Error, "x", raw)
            .with_did_you_mean(raw, "name");
        let out = rebase_to_file(&db, second, [d]);
        assert_eq!(out[0].span.start, raw.start + base);
        assert_eq!(
            out[0]
                .did_you_mean
                .as_ref()
                .expect("did_you_mean survives")
                .wrong_span
                .start,
            raw.start + base
        );
    }

    /// Helper — return the mapping's own text (the substring of `src`
    /// covered by `mapping_cst_node`). Spans are mapping-relative, so the
    /// tests slice into this string, NOT into the full file source. Per
    /// the offset-semantics doc-comment at the top of this module.
    fn mapping_text<'db>(db: &'db fossil_base::FossilDb, m: MappingLoc<'db>) -> String {
        let cst = mapping_cst_node(db, m);
        cst.syntax()
            .expect("mapping CST node must exist")
            .text()
            .to_string()
    }

    /// Property 0 of `hello.fossil` is the iri template property. Phase 2
    /// synthesises a `Template` RHS for it. The recorded mapping-relative
    /// span MUST cover the entire RHS expression (backtick to backtick
    /// inclusive) and be non-zero width.
    #[test]
    fn spans_for_template_property() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";
        let (db, file) = db_with_text(SRC, "tpl.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        let span = s
            .get(&db, ExprId(0))
            .expect("property 0 (iri = template) must have a recorded span");
        assert!(
            span.end > span.start,
            "Template span must be non-zero width: {span:?}"
        );
        // Spans are mapping-relative; slice into the mapping's own text.
        let text = mapping_text(&db, m);
        let extracted = &text[span.start as usize..span.end as usize];
        assert!(
            extracted.starts_with('`'),
            "Template span must start at the opening backtick, got {extracted:?}"
        );
        assert!(
            extracted.ends_with('`'),
            "Template span must end at the closing backtick, got {extracted:?}"
        );
        assert!(
            extracted.contains("${.id}"),
            "Template span must cover the full backtick-delimited body, got {extracted:?}"
        );
    }

    /// Property 1 of `hello.fossil` is `ex:name = .name` — a `FieldRef`
    /// RHS. The recorded mapping-relative span MUST cover `.name`
    /// (5 chars including the leading dot) and be non-zero width.
    #[test]
    fn spans_for_field_ref() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";
        let (db, file) = db_with_text(SRC, "fref.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        let span = s
            .get(&db, ExprId(1))
            .expect("property 1 (ex:name = .name) must have a recorded span");
        assert!(
            span.end > span.start,
            "FieldRef span must be non-zero width: {span:?}"
        );
        let text = mapping_text(&db, m);
        let extracted = &text[span.start as usize..span.end as usize];
        assert_eq!(
            extracted, ".name",
            "FieldRef span must cover exactly `.name`, got {extracted:?}"
        );
    }

    /// `ex:link = ex:Foo` exercises the `IRI_EXPR` prefixed-name RHS form
    /// (the plan 03-01 fix). The recorded mapping-relative span MUST
    /// cover the 6 characters of `ex:Foo` exactly.
    #[test]
    fn spans_for_prefixed_name_rhs() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:link = ex:Foo
";
        let (db, file) = db_with_text(SRC, "pn.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        let span = s
            .get(&db, ExprId(1))
            .expect("property 1 (ex:link = ex:Foo) must have a recorded span");
        assert!(
            span.end > span.start,
            "PrefixedName span must be non-zero width: {span:?}"
        );
        let text = mapping_text(&db, m);
        let extracted = &text[span.start as usize..span.end as usize];
        assert_eq!(
            extracted, "ex:Foo",
            "PrefixedName span must cover `ex:Foo` exactly, got {extracted:?}"
        );
    }

    /// `ex:greeting = "Alice"` exercises the `StringLit` RHS form. The
    /// recorded mapping-relative span MUST cover the literal INCLUDING
    /// the surrounding quotes — the `EXPR` CST node's `text_range()` is
    /// the lex span, which is quote-inclusive.
    #[test]
    fn spans_for_string_lit_property() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:greeting = \"Alice\"
";
        let (db, file) = db_with_text(SRC, "lit.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        let span = s
            .get(&db, ExprId(1))
            .expect("property 1 (ex:greeting = \"Alice\") must have a recorded span");
        assert!(
            span.end > span.start,
            "StringLit span must be non-zero width: {span:?}"
        );
        let text = mapping_text(&db, m);
        let extracted = &text[span.start as usize..span.end as usize];
        assert_eq!(
            extracted, "\"Alice\"",
            "StringLit span must cover the quoted literal including quotes, \
             got {extracted:?}"
        );
    }

    /// Sanity: `Spans::get` returns `None` for an unknown `ExprId` rather
    /// than panicking. Phase 3 callers (the bidirectional checker, hover
    /// handler) lean on this for graceful degradation.
    #[test]
    fn spans_get_returns_none_for_unknown_expr_id() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
";
        let (db, file) = db_with_text(SRC, "unk.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        // Only one property → ExprId(0). ExprId(99) must return None.
        assert!(s.get(&db, ExprId(0)).is_some());
        assert!(s.get(&db, ExprId(99)).is_none());
    }

    /// Memoisation sanity check — `Spans` is salsa-tracked, so repeated
    /// invocations on the same `MappingLoc` yield the same handle.
    #[test]
    fn spans_is_memoised_per_mapping() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";
        let (db, file) = db_with_text(SRC, "memo.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let a = spans(&db, m);
        let b = spans(&db, m);
        assert_eq!(a, b);
    }
}
