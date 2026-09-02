//! [`ItemTree`] — signatures-only top-level summary of a `.fossil` file.
//!
//! This is the upper half of the invalidation-barrier pattern: an
//! `ItemTree` carries SIGNATURES ONLY, never body content, so nothing that
//! depends on it can be invalidated by an edit inside a mapping.
//! [`crate::body::body`] is the lower half:
//! per-mapping body content lives behind that separate query, so editing one
//! mapping's body does NOT invalidate `item_tree(file)`.
//!
//! Pattern: rust-analyzer `crates/hir-def/src/item_tree.rs` (lines ~216-224
//! in the reference revision).
//!
//! # CRITICAL INVARIANT
//!
//! Header extractors in this module MUST NOT inspect property values, body
//! expression text, or any subtree below the signature-bearing region. They
//! read header tokens and structural counts (e.g. `body_property_count`)
//! only. `tests/invalidation_regression.rs` enforces this mechanically via a
//! Salsa event-count regression test: editing one character in the body of
//! mapping #3 in a 10-mapping fixture must re-execute the per-mapping queries
//! for that mapping and no sibling's.
//!
//! Counting properties IS allowed (and required) — adding or removing a
//! property is a structural change and SHOULD invalidate `item_tree`.
//! Editing the value of an existing property is NOT structural and MUST
//! NOT.

use crate::ast_id::{FileAstId, MappingNode, SourceDefNode, ast_id_map};
use fossil_base::SourceFile;
use fossil_syntax::{SyntaxKind, SyntaxNode};
use smol_str::SmolStr;

/// Per-file signatures table. Stable across body-only edits.
#[salsa::tracked(debug)]
pub struct ItemTree<'db> {
    #[returns(ref)]
    pub items: Vec<ItemHeader>,
}

/// One signature entry. Each variant carries header data and (for mappings)
/// structural counts — NEVER body expression content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ItemHeader {
    SourceDef(SourceDefHeader),
    Mapping(MappingHeader),
}

// A `PrefixDecl(PrefixDeclHeader)` variant carried a `(name, iri)` pair here.
// The vocabulary declaration is gone — a program writes full IRIs inside
// interpolated strings and short names everywhere else — and so is its node.

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceDefHeader {
    pub ast_id: FileAstId<SourceDefNode>,
    pub name: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct MappingHeader {
    pub ast_id: FileAstId<MappingNode>,
    /// `User` in `User : Person from users`.
    pub name: SmolStr,
    /// Source binding name from the `from` clause if it's a simple `IDENT`;
    /// `None` if the source is a complex `Expression` — a complex source is
    /// resolved through the body query, never from the signature.
    pub source_binding: Option<SmolStr>,
    /// Number of `PROPERTY` children in `MAPPING_BODY`. Counting only — the
    /// per-property contents live in [`crate::body::HirBody::properties`].
    pub body_property_count: u32,
}

// There was an `ImportHeader` here — the `use foo/bar as baz` signature. It
// carried a path and an alias and nothing ever read either: a file is compiled
// alone, so there is no second file for a path to name. It went with the
// `use` form itself.

/// Build the per-file `ItemTree` by walking top-level CST children once and
/// extracting each one's signature-only summary.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn item_tree<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> ItemTree<'db> {
    let cst = fossil_syntax::parse(db, file);
    // Co-depend on `ast_id_map` so it ends up cached — the WAVE 4 invalidation
    // test asserts both queries share the same input dependency surface.
    let _ = ast_id_map(db, file);
    let mut items: Vec<ItemHeader> = Vec::new();
    for (idx, child) in cst.root(db).syntax().children().enumerate() {
        let ast_id_raw = u32::try_from(idx).expect("file with > u32::MAX top-level items");
        match child.kind() {
            SyntaxKind::SOURCE_DEF => {
                if let Some(h) = extract_source_def_header(&child, FileAstId::new(ast_id_raw)) {
                    items.push(ItemHeader::SourceDef(h));
                }
            }
            SyntaxKind::MAPPING => {
                if let Some(h) = extract_mapping_header(&child, FileAstId::new(ast_id_raw)) {
                    items.push(ItemHeader::Mapping(h));
                }
            }
            _ => {} // trivia, ERROR nodes — ignored at the item level
        }
    }
    ItemTree::new(db, items)
}

// ───────── Header extractors ──────────────────────────────────────────────
// CRITICAL: each extractor reads ONLY header tokens (and counts PROPERTY
// children for `body_property_count`). NONE of them iterates the EXPR / value
// subtree of any PROPERTY. That's the invalidation barrier.

fn extract_source_def_header(
    node: &SyntaxNode,
    ast_id: FileAstId<SourceDefNode>,
) -> Option<SourceDefHeader> {
    let name = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    Some(SourceDefHeader {
        ast_id,
        name: SmolStr::from(name.text()),
    })
}

fn extract_mapping_header(
    node: &SyntaxNode,
    ast_id: FileAstId<MappingNode>,
) -> Option<MappingHeader> {
    let header = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)?;
    let body = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_BODY);

    // Mapping name: first direct IDENT child token of MAPPING_HEADER.
    let name_tok = header
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    let name = SmolStr::from(name_tok.text());

    // There was a `shape_iris: Vec<SmolStr>` collected here, one entry per
    // IRI_EXPR in the SHAPE_EXPR, because a shape could be an intersection.
    // Nothing ever read the field, and the intersection is gone: a header names
    // one shape, and `lower_to_hir` resolves that name against `def_map`'s type
    // bindings, which this query deliberately does not depend on.

    // Source binding: scan tokens after KW_FROM for a single IDENT. Complex
    // expressions are signalled as `None` and are resolved through the body
    // query path.
    let mut source_binding = None;
    let mut after_from = false;
    for tok in header
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
    {
        if tok.kind() == SyntaxKind::KW_FROM {
            after_from = true;
            continue;
        }
        if after_from && tok.kind() == SyntaxKind::IDENT {
            source_binding = Some(SmolStr::from(tok.text()));
            break;
        }
    }
    // The parser wraps the from-source in `EXPR > LITERAL_EXPR > IDENT` —
    // peek into that shape if the direct-token scan above didn't find one.
    if source_binding.is_none()
        && let Some(expr_node) = header.children().find(|c| c.kind() == SyntaxKind::EXPR)
        && let Some(ident) = expr_node
            .descendants_with_tokens()
            .filter_map(fossil_syntax::SyntaxElement::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT)
    {
        source_binding = Some(SmolStr::from(ident.text()));
    }

    // Body property count — structural signal. DOES contribute to ItemTree
    // input (adding/removing a property is a structural change). Property
    // VALUES are not read — that's body's job.
    let body_property_count = body.map_or(0, |b| {
        u32::try_from(
            b.children()
                .filter(|c| c.kind() == SyntaxKind::PROPERTY)
                .count(),
        )
        .unwrap_or(u32::MAX)
    });

    Some(MappingHeader {
        ast_id,
        name,
        source_binding,
        body_property_count,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
type { Person } := io.shex(\"personas.shex\")

users := io.csv(\"examples/users.csv\")

User : Person from users
    @subject = \"https://example.org/user/{users.id}\"
    name = users.name
";

    #[test]
    fn item_tree_for_hello_fossil_has_two_items_signatures_only() {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, HELLO_FOSSIL.to_string(), "hello.fossil".to_string());
        let it = item_tree(&db, file);
        let items = it.items(&db);
        // TWO, not three: the `type { … } := …` binding that replaced the
        // `prefix` line has no `ItemHeader` variant, because nothing downstream
        // reads a type binding's SIGNATURE — `def_map` reads the binding whole.
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], ItemHeader::SourceDef(_)));
        match &items[1] {
            ItemHeader::Mapping(h) => {
                assert_eq!(h.name.as_str(), "User");
                assert_eq!(h.body_property_count, 2);
                assert_eq!(h.source_binding.as_deref(), Some("users"));
            }
            other @ ItemHeader::SourceDef(_) => panic!("expected Mapping, got {other:?}"),
        }
    }

    /// Compile-time + runtime guarantee that `ItemHeader::Mapping` does not
    /// carry property values. Editing a property's right-hand side
    /// (`users.a` → `users.b`) leaves the structural signal (count, names,
    /// shape) unchanged.
    #[test]
    fn item_tree_excludes_body() {
        let src_a = "\
type { Person } := io.shex(\"personas.shex\")
users := io.csv(\"x.csv\")
User : Person from users
    a = users.a
";
        let src_b = "\
type { Person } := io.shex(\"personas.shex\")
users := io.csv(\"x.csv\")
User : Person from users
    a = users.b
"; // users.a → users.b — body-only edit
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db_a = fossil_base::FossilDb::new(system.clone());
        let db_b = fossil_base::FossilDb::new(system);
        let f_a = fossil_base::SourceFile::new(&db_a, src_a.to_string(), "x.fossil".to_string());
        let f_b = fossil_base::SourceFile::new(&db_b, src_b.to_string(), "x.fossil".to_string());
        let it_a = item_tree(&db_a, f_a);
        let it_b = item_tree(&db_b, f_b);
        let items_a = it_a.items(&db_a);
        let items_b = it_b.items(&db_b);
        assert_eq!(items_a.len(), items_b.len());
        // Index 1, not 2: the `type { … } := …` binding that replaced the
        // `prefix` line contributes no `ItemHeader`, so the mapping follows the
        // one `SourceDef` directly. See the count above.
        let m_a = match &items_a[1] {
            ItemHeader::Mapping(m) => m,
            other @ ItemHeader::SourceDef(_) => {
                panic!("expected Mapping in items_a[1], got {other:?}")
            }
        };
        let m_b = match &items_b[1] {
            ItemHeader::Mapping(m) => m,
            other @ ItemHeader::SourceDef(_) => {
                panic!("expected Mapping in items_b[1], got {other:?}")
            }
        };
        assert_eq!(m_a.name, m_b.name);
        assert_eq!(m_a.body_property_count, m_b.body_property_count);
        assert_eq!(m_a.source_binding, m_b.source_binding);
    }
}
