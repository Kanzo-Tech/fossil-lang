//! [`HirBody`] — per-mapping body content + the [`body`] Salsa query.
//!
//! Lower half of the invalidation-barrier pattern.
//! [`crate::item_tree::ItemTree`] is the SIGNATURE table; this module is the
//! BODY table. They are deliberately separate `#[salsa::tracked]` queries
//! with separate input-dependency surfaces.
//!
//! - `item_tree(db, file)` reads top-level header tokens + structural counts
//!   only.
//! - `body(db, mapping)` reads the body of ONE mapping.
//!
//! Editing one mapping's body invalidates exactly that mapping's
//! `body(M_k)` (and its downstream type-check / MIR / codegen queries).
//! Sibling mappings stay cached.
//!
//! # `nth(idx)` correctness — CRITICAL
//!
//! [`body`] resolves the target mapping via [`mapping_cst_node`]'s filter-
//! before-nth lookup over MAPPING-kind CST children. `.filter(...)` MUST
//! come BEFORE `.nth(idx)`: without it, `PREFIX_DECL` / `SOURCE_DEF` /
//! `IMPORT` top-level children inflate the count and `.nth(idx)` returns the
//! wrong node. The regression test
//! `tests::body_filters_to_mapping_kind_before_indexing` enforces this.
//!
//! # The per-mapping invalidation barrier — CRITICAL
//!
//! [`body`] does NOT depend on `fossil_syntax::parse(db, file)` directly.
//! Doing so would tie every per-mapping body query to the whole file's CST,
//! causing all 10 body queries in a 10-mapping file to re-execute on any
//! body edit (because `parse()` returns a new `Cst` tracked struct whose
//! `root: CstRoot` field changes structurally on any byte edit). The
//! invalidation regression test
//! `crates/fossil-hir/tests/invalidation_regression.rs` exists exactly to
//! catch this regression.
//!
//! The fix: [`mapping_cst_node`] is an intermediate per-mapping CST-
//! extraction Salsa query that returns just the green subtree for one
//! MAPPING. Because rowan's `GreenNode` interner reuses subtree Arcs
//! across edits to UNAFFECTED siblings, the green subtree for mapping #1
//! is bit-identical (same Arc) before and after an edit to mapping #3.
//! Salsa's structural equality check on [`MappingCstNode`] (which
//! delegates to `GreenNode::eq` → rowan structural-tree equality →
//! Arc-pointer equality on shared subtrees) returns "no change", so
//! downstream [`body`] queries for unaffected mappings validate via
//! `DidValidateMemoizedValue` instead of re-executing.
//!
//! This is the rust-analyzer per-item Salsa fan-out pattern; the
//! intermediate query is the data-layout fix, and it is what makes the
//! barrier structural rather than something every caller has to remember.

use crate::def_map::{MappingLoc, def_map};
use crate::lower::{HirProperty, lower_property_public};
use fossil_syntax::{SyntaxKind, SyntaxNode};
use rowan::GreenNode;

/// Stable per-mapping expression id. Indexed into the body's expression
/// arena, and the second half of the key `(MappingLoc, ExprId)` the
/// provenance and span side tables are both keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct ExprId(pub u32);

/// Per-mapping body content.
///
/// Owns the flat `Vec<HirProperty>` that `HirMapping.properties`
/// once carried (the field is REMOVED from `HirMapping`: the signature
/// table carries names and structural counts and never body content, so a body
/// edit cannot invalidate it). [`Self::expr_count`] is the `ExprId` arena's
/// bookkeeping.
///
/// # `ExprId` is assigned HERE, and nowhere else
///
/// [`Self::expr_spans`] is the reason. There were THREE notions of `ExprId` in
/// the tree and they disagreed the moment one property failed to lower:
///
/// 1. this query — a DENSE index over the properties that lowered;
/// 2. `crate::spans::spans` — the property's POSITION among the CST's
///    `PROPERTY` children, from an `.enumerate()`, with a comment claiming it
///    matched this one;
/// 3. `fossil_ide::hover` — the CST position again, used both as the `ExprId`
///    and to index this `properties` vector.
///
/// A property that parses and does not lower — a CURIE key, `iri =`, an
/// absolute-IRI key: every retired spelling in the corpus is exactly one of
/// these — makes the CST longer than the HIR, and everything after it shifts.
/// The consequences were silent in all three directions: the checker numbers
/// with (1) and looks the span up in (2), so a type error UNDERLINES THE WRONG
/// PROPERTY; hover reads the short name and the predicate IRI of a different
/// property than the one under the cursor.
///
/// So the arena publishes its own spans, in lockstep with `properties`, from
/// the `prop_node` it already has in hand. `spans()` is an accessor over this
/// vector rather than a second walk of the CST, and hover resolves a position
/// by SPAN CONTAINMENT instead of by counting. One walk, one numbering.
///
/// A side table and not a field on `HirProperty`: a `span` on the
/// property would enter its `PartialEq`/`Hash` and two source-identical
/// properties at different offsets would stop deduping.
#[salsa::tracked(debug)]
pub struct HirBody<'db> {
    /// Per-mapping flat property list, in source order.
    #[returns(ref)]
    pub properties: Vec<HirProperty>,
    /// One mapping-relative span per lowered property, at the SAME index —
    /// `expr_spans[i]` is the source range of `properties[i]`'s right-hand
    /// side. See the type docs: this is what makes `ExprId` one thing.
    ///
    /// Mapping-relative, because it is read off [`mapping_cst_node`]'s detached
    /// subtree (rowan resets offsets to zero at a new root). The
    /// [`crate::spans::rebase_to_file`] direction is unchanged.
    #[returns(ref)]
    pub expr_spans: Vec<fossil_base::Span>,
    /// Count of distinct expression nodes lowered for this mapping. The
    /// provenance side table is keyed by `(MappingLoc, ExprId)` for the ids
    /// in `0..expr_count`.
    pub expr_count: u32,
}

/// Salsa-storable handle to a per-mapping CST subtree.
///
/// Holds just the MAPPING node, not the whole file. Wraps a
/// [`rowan::GreenNode`] so the [`mapping_cst_node`] Salsa query can store
/// and structurally compare per-mapping subtrees.
///
/// Why this exists: editing one mapping's body changes the WHOLE file's
/// `Cst` (because rowan rebuilds the root green node up the spine to the
/// edit site). If [`body`] depended on `parse(db, file)`'s `Cst` directly,
/// EVERY per-mapping body query would see a different `Cst` input and
/// re-execute. By inserting [`mapping_cst_node`] between `parse` and `body`,
/// per-mapping bodies depend on a Salsa-tracked value that's STRUCTURALLY
/// EQUAL for sibling mappings (rowan reuses subtree `Arc`s across edits to
/// unaffected siblings; the structural-equality check on `GreenNode`
/// returns `true` even if the Arcs aren't pointer-equal). Salsa's
/// `maybe_update` returns `false` for unchanged values, so downstream
/// queries validate via `DidValidateMemoizedValue` instead of re-executing.
///
/// This is the rust-analyzer per-item Salsa fan-out pattern. The invariant
/// is enforced by the invalidation regression test at
/// `crates/fossil-hir/tests/invalidation_regression.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MappingCstNode {
    /// The per-mapping green node, or `None` if the mapping index didn't
    /// resolve to a MAPPING-kind CST child.
    green: Option<GreenNode>,
}

// SAFETY: third-party-trait integration boundary. Salsa's
// `Update` trait is `unsafe` by design — implementations must guarantee
// `maybe_update` correctly determines whether the new value differs. We
// delegate to `PartialEq` on `Option<GreenNode>`, which rowan implements
// as structural tree equality on the inner node. No safe alternative
// because Salsa requires `unsafe impl` even for trivially-safe bodies.
#[allow(unsafe_code)]
unsafe impl salsa::Update for MappingCstNode {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: caller guarantees `old_pointer` is a valid, aligned
        // pointer to an initialised `MappingCstNode` owned by Salsa storage
        // (Salsa contract).
        let old = unsafe { &mut *old_pointer };
        if *old == new_value {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

impl MappingCstNode {
    /// Reconstruct the red [`SyntaxNode`] view from the stored green node.
    /// Returns `None` if the mapping index didn't resolve.
    #[must_use]
    pub fn syntax(&self) -> Option<SyntaxNode> {
        self.green.as_ref().map(|g| SyntaxNode::new_root(g.clone()))
    }
}

/// Per-mapping CST extraction query. Resolves a [`MappingLoc`] to its
/// owned [`GreenNode`] subtree (just the MAPPING node, not the whole file).
///
/// This is the invalidation barrier between `parse(db, file)` (which
/// re-executes on any byte edit) and [`body`] (which depends only on the
/// per-mapping CST subtree, stable across sibling-mapping edits).
///
/// Resolution uses filter-then-nth over MAPPING-kind top-level children, and
/// `def_map`'s `MappingLoc.index` MUST allocate with the same convention — a
/// dense position among MAPPING-kind children, never an all-children index.
/// If the two sides ever disagree this query silently hands back another
/// mapping's subtree, with no panic anywhere downstream.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn mapping_cst_node<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> MappingCstNode {
    let file = mapping.file(db);
    let idx = mapping.index(db);
    let cst = fossil_syntax::parse(db, file);

    // CRITICAL: filter to MAPPING-kind BEFORE `.nth(idx)`. See the module-
    // level comment + the `body_filters_to_mapping_kind_before_indexing`
    // regression test. The previous (buggy) form was
    //   cst.root(db).syntax().children().nth(idx).filter(|n| n.kind() == MAPPING)
    // which silently returns the wrong node when a file has non-MAPPING
    // top-level siblings before the target mapping.
    let mapping_node = cst
        .root(db)
        .syntax()
        .children()
        .filter(|n| n.kind() == SyntaxKind::MAPPING)
        .nth(idx);

    let green = mapping_node.map(|n| n.green().into_owned());
    MappingCstNode { green }
}

/// Per-mapping body query. Keyed by the interned [`MappingLoc`]; depends on
/// the per-mapping CST subtree (via [`mapping_cst_node`]) and the file's
/// prefix table (via [`def_map`]). Body content is read from EXACTLY ONE
/// mapping — editing a sibling mapping's body does NOT invalidate
/// `body(M_k)` because [`mapping_cst_node`] is the invalidation barrier
/// (rowan's subtree Arc reuse → structural equality → Salsa
/// `DidValidateMemoizedValue` instead of re-execution).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn body<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> HirBody<'db> {
    let mapping_cst = mapping_cst_node(db, mapping);
    // The names a `type { … } := …` binding introduced, for the one decision the
    // lowering cannot make from the CST alone: `Person(User.email)` and
    // `str.slug(x)` are the same node, and only a bound type name makes the
    // first an edge. `def_map` is FILE-keyed and structurally
    // stable across body-only edits, so this read adds no per-mapping fan-out —
    // the same argument that lets `lower_to_mir_pg` read it.
    let type_names: Vec<smol_str::SmolStr> = def_map(db, mapping.file(db))
        .types(db)
        .iter()
        .map(|t| t.name.clone())
        .collect();

    let mut properties: Vec<HirProperty> = Vec::new();
    let mut expr_spans: Vec<fossil_base::Span> = Vec::new();
    let mut expr_count: u32 = 0;
    // Where the identity was written, among the properties that lowered. Its
    // three obligations — required, exactly one, first — are checked here and
    // not in the parser: each is a fact about a mapping
    // rather than about a token, and the message wants the mapping's name.
    let mut subjects: Vec<(usize, fossil_base::Span)> = Vec::new();
    if let Some(node) = mapping_cst.syntax() {
        let name = mapping_name(&node);
        if let Some(body_node) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)
        {
            for prop_node in body_node
                .children()
                .filter(|c| c.kind() == SyntaxKind::PROPERTY)
            {
                if let Some(prop) = lower_property_public(db, &prop_node, &type_names) {
                    if matches!(prop.key, crate::lower::PropertyKey::Subject) {
                        let r = prop_node.text_range();
                        subjects.push((
                            properties.len(),
                            fossil_base::Span::new(r.start().into(), r.end().into()),
                        ));
                    }
                    expr_spans.push(rhs_span(&prop_node));
                    properties.push(prop);
                    expr_count = expr_count.saturating_add(1);
                }
            }
            check_identity(db, &name, &subjects, &body_node);
        }
    }
    HirBody::new(db, properties, expr_spans, expr_count)
}

/// The span a diagnostic about this property's VALUE should underline.
///
/// `EXPR`'s own `text_range()` swallows the leading whitespace and the trailing
/// newline, so it descends to the first child — the tight range over the
/// right-hand side's tokens. A property that lowered without one falls back to
/// the whole `PROPERTY` node, which is imprecise and inside the right line;
/// what it must NOT do is skip, because the index is the `ExprId`.
fn rhs_span(prop_node: &SyntaxNode) -> fossil_base::Span {
    let range = prop_node
        .children()
        .find(|c| c.kind() == SyntaxKind::EXPR)
        .and_then(|expr| expr.children().next())
        .map_or_else(|| prop_node.text_range(), |inner| inner.text_range());
    fossil_base::Span::new(range.start().into(), range.end().into())
}

/// The mapping's own name, read off its header. Used only in the identity
/// diagnostics below, which is why it is read from the CST rather than from
/// `lower_to_hir`: a file-keyed read here would re-lower every body in the file
/// whenever any header changed, to put one word in one message.
fn mapping_name(mapping_node: &SyntaxNode) -> String {
    mapping_node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)
        .and_then(|h| {
            h.children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::IDENT)
                .map(|t| t.text().to_string())
        })
        .unwrap_or_else(|| "this mapping".to_string())
}

/// `MappingBody := SubjectAssign Property+` — required, exactly one, first.
///
/// All three used to be unchecked, and the first one was the expensive silence:
/// a mapping with no identity lowered to vertices whose subject was the empty
/// string, which dedups every row of the mapping into one blank node. That was
/// caught downstream in `fossil-mir` as a `delay_span_bug` — an internal-error
/// spelling for something the author wrote and can fix.
fn check_identity(
    db: &dyn fossil_base::Db,
    name: &str,
    subjects: &[(usize, fossil_base::Span)],
    body_node: &SyntaxNode,
) {
    use salsa::Accumulator as _;
    let emit = |span: fossil_base::Span, message: String| {
        fossil_base::Diagnostic::new(fossil_base::Severity::Error, message, span).accumulate(db);
    };
    let Some(&(index, first_span)) = subjects.first() else {
        let r = body_node.text_range();
        emit(
            fossil_base::Span::new(r.start().into(), r.end().into()),
            format!(
                "`{name}` declares no `@subject`, so the rows it writes have no identity. The \
                 first line of a mapping body is `@subject = <expr>`."
            ),
        );
        return;
    };
    if let Some(&(_, second_span)) = subjects.get(1) {
        emit(
            second_span,
            format!(
                "`{name}` declares `@subject` twice, and a type has one identity. Every mapping \
                 that produces this type writes the same one, so that an edge naming the type \
                 reaches the same node whichever mapping emitted it."
            ),
        );
    }
    if index != 0 {
        emit(
            first_span,
            format!(
                "`@subject` is the first line of a mapping body, and in `{name}` it is line \
                 {}. The identity comes before what it identifies.",
                index + 1
            ),
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO: &str = "\
type { Person } := io.shex(\"personas.shex\")
users := io.csv(\"x.csv\")
User : Person from users
    @subject = \"https://example.org/u/{users.id}\"
    name = users.name
";

    #[test]
    fn body_returns_two_properties_for_hello() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
        let dm = def_map(&db, file);
        let m = dm.mappings(&db)[0];
        let b = body(&db, m);
        assert_eq!(b.properties(&db).len(), 2);
        assert_eq!(b.expr_count(&db), 2);
    }

    /// Regression test for the wrong-body bug (plan-checker Blocker 2).
    ///
    /// A file with `type binding + source_def + 3 mappings` MUST return the
    /// right mapping for each `body(M_i)` call. The three mappings have
    /// distinct property counts (3, 2, 1), so an off-by-N indexing bug
    /// surfaces as the wrong property count. Specifically:
    ///
    /// - `body(M_0)` returns `Mapping_A` (3 properties)
    /// - `body(M_1)` returns `Mapping_B` (2 properties)
    /// - `body(M_2)` returns `Mapping_C` (1 property)
    ///
    /// The unfiltered `.nth(2)` form would return `Mapping_A` for `body(M_2)`
    /// (the 3rd top-level child is `mapping_0` because the `type` binding +
    /// `source_def` occupy positions 0 and 1), so the wrong test would see 3
    /// properties where 1 is expected. That's the silent corruption the
    /// filter-before-nth ordering prevents.
    ///
    /// The two non-mapping children were `prefix ex: <…>` + the source; the
    /// `type { … } := …` binding is what stands in front of the mappings now,
    /// and it has to, or this asserts nothing about filtering. `@subject` is
    /// one of the counted properties — the identity is a property with a
    /// language-owned key, so `Mapping_C`'s single property IS its identity.
    #[test]
    fn body_filters_to_mapping_kind_before_indexing() {
        let src = "\
type { Shape } := io.shex(\"s.shex\")

users := io.csv(\"x.csv\")

Mapping_A : Shape from users
    @subject = \"https://example.org/a/{users.a}\"
    b = users.b
    c = users.c

Mapping_B : Shape from users
    @subject = \"https://example.org/b/{users.a}\"
    b = users.b

Mapping_C : Shape from users
    @subject = \"https://example.org/c/{users.c}\"
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, src.to_string(), "indexing.fossil".to_string());
        let dm = def_map(&db, file);
        let mappings = dm.mappings(&db);
        assert_eq!(
            mappings.len(),
            3,
            "expected 3 MappingLocs for 3 MAPPINGs (got {})",
            mappings.len()
        );

        let b_a = body(&db, mappings[0]);
        let b_b = body(&db, mappings[1]);
        let b_c = body(&db, mappings[2]);
        assert_eq!(
            b_a.properties(&db).len(),
            3,
            "Mapping_A must have 3 properties"
        );
        assert_eq!(
            b_b.properties(&db).len(),
            2,
            "Mapping_B must have 2 properties"
        );
        assert_eq!(
            b_c.properties(&db).len(),
            1,
            "Mapping_C must have 1 property (NOT 3 — that would be the \
             unfiltered .nth(2) bug returning Mapping_A's body)"
        );
    }

    #[test]
    fn body_is_memoised_per_mapping() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
        let dm = def_map(&db, file);
        let m = dm.mappings(&db)[0];
        let a = body(&db, m);
        let b = body(&db, m);
        assert_eq!(a, b);
    }
}
