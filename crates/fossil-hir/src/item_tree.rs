//! [`ItemTree`] — signatures-only top-level summary of a `.fossil` file.
//!
//! This is the upper half of the CORE-02 invalidation-barrier pattern (see
//! ADR-0005 for the full decision). [`crate::body::body`] is the lower half:
//! per-mapping body content lives behind that separate query, so editing one
//! mapping's body does NOT invalidate `item_tree(file)`.
//!
//! Pattern: rust-analyzer `crates/hir-def/src/item_tree.rs` (lines ~216-224
//! in the reference revision).
//!
//! # CRITICAL INVARIANT (CORE-02 SC#2)
//!
//! Header extractors in this module MUST NOT inspect property values, body
//! expression text, or any subtree below the signature-bearing region. They
//! read header tokens and structural counts (e.g. `body_property_count`)
//! only. Wave 4 plan 02-07 enforces this mechanically via a Salsa event-
//! count regression test: editing one character in the body of mapping #3
//! in a 10-mapping fixture must produce ≤4 `WillExecute` events.
//!
//! Counting properties IS allowed (and required) — adding or removing a
//! property is a structural change and SHOULD invalidate `item_tree`.
//! Editing the value of an existing property is NOT structural and MUST
//! NOT.

use crate::ast_id::{
    DefinitionNode, FileAstId, ImportNode, MappingNode, PrefixDeclNode, SourceDefNode, ast_id_map,
};
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
    PrefixDecl(PrefixDeclHeader),
    SourceDef(SourceDefHeader),
    Mapping(MappingHeader),
    Definition(DefinitionHeader),
    ExportedDefinition(ExportedDefinitionHeader),
    Import(ImportHeader),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PrefixDeclHeader {
    pub ast_id: FileAstId<PrefixDeclNode>,
    pub name: SmolStr,
    pub iri: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceDefHeader {
    pub ast_id: FileAstId<SourceDefNode>,
    pub name: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct MappingHeader {
    pub ast_id: FileAstId<MappingNode>,
    /// `User` in `User : ex:Person from users`.
    pub name: SmolStr,
    /// Surface text of each `IRI_EXPR` in the `SHAPE_EXPR`. For a shape
    /// intersection `A & B` this carries both elements in source order.
    /// Resolution to fully-qualified IRIs is the responsibility of the
    /// downstream HIR `lower_to_hir` query — keeping `ItemTree`'s view at
    /// "surface text" means `item_tree(file)` does not depend on the prefix
    /// table (which is computed by `def_map`).
    pub shape_iris: Vec<SmolStr>,
    /// Source binding name from the `from` clause if it's a simple `IDENT`;
    /// `None` if the source is a complex `Expression`. Phase 3 handles
    /// complex sources.
    pub source_binding: Option<SmolStr>,
    /// Number of `PROPERTY` children in `MAPPING_BODY`. Counting only — the
    /// per-property contents live in [`crate::body::HirBody::properties`].
    pub body_property_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct DefinitionHeader {
    pub ast_id: FileAstId<DefinitionNode>,
    pub name: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ExportedDefinitionHeader {
    pub inner: DefinitionHeader,
    /// `true` if a `TYPE_ANNOTATION` subnode was present on the
    /// `EXPORTED_DEFINITION` node. Phase 3 type-checker uses this signal.
    pub has_type_annotation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ImportHeader {
    pub ast_id: FileAstId<ImportNode>,
    /// Joined path text (e.g. `stdlib/seq`). Phase 3 splits this into
    /// segments.
    pub path: SmolStr,
    /// Alias from `as IDENT` if present.
    pub alias: Option<SmolStr>,
}

/// Build the per-file `ItemTree` by walking top-level CST children once and
/// extracting each one's signature-only summary.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn item_tree<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> ItemTree<'db> {
    let cst = fossil_syntax::parse(db, file);
    // Co-depend on `ast_id_map` so it ends up cached — the WAVE 4 invalidation
    // test asserts both queries share the same input dependency surface.
    let _ = ast_id_map(db, file);
    let mut items: Vec<ItemHeader> = Vec::new();
    for (idx, child) in cst.root(db).syntax().children().enumerate() {
        let ast_id_raw = u32::try_from(idx).expect("file with > u32::MAX top-level items");
        match child.kind() {
            SyntaxKind::PREFIX_DECL => {
                if let Some(h) = extract_prefix_decl_header(&child, FileAstId::new(ast_id_raw)) {
                    items.push(ItemHeader::PrefixDecl(h));
                }
            }
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
            SyntaxKind::IMPORT => {
                if let Some(h) = extract_import_header(&child, FileAstId::new(ast_id_raw)) {
                    items.push(ItemHeader::Import(h));
                }
            }
            SyntaxKind::DEFINITION => {
                if let Some(h) = extract_definition_header(&child, FileAstId::new(ast_id_raw)) {
                    items.push(ItemHeader::Definition(h));
                }
            }
            SyntaxKind::EXPORTED_DEFINITION => {
                if let Some(h) =
                    extract_exported_definition_header(&child, FileAstId::new(ast_id_raw))
                {
                    items.push(ItemHeader::ExportedDefinition(h));
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

fn extract_prefix_decl_header(
    node: &SyntaxNode,
    ast_id: FileAstId<PrefixDeclNode>,
) -> Option<PrefixDeclHeader> {
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect();
    // Pattern: KW_PREFIX IDENT SHAPE_SEP ABS_IRI
    let name = toks.iter().find(|t| t.kind() == SyntaxKind::IDENT)?;
    let iri = toks.iter().find(|t| t.kind() == SyntaxKind::ABS_IRI)?;
    let iri_text = iri.text().trim_start_matches('<').trim_end_matches('>');
    Some(PrefixDeclHeader {
        ast_id,
        name: SmolStr::from(name.text()),
        iri: SmolStr::from(iri_text),
    })
}

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

    // Shape IRIs: each IRI_EXPR inside SHAPE_EXPR carries one shape. Surface
    // text is captured (no prefix expansion at the ItemTree level — that's
    // a downstream concern that depends on def_map's prefix table).
    let mut shape_iris: Vec<SmolStr> = Vec::new();
    if let Some(shape_expr) = header
        .children()
        .find(|c| c.kind() == SyntaxKind::SHAPE_EXPR)
    {
        for iri_node in shape_expr
            .children()
            .filter(|c| c.kind() == SyntaxKind::IRI_EXPR)
        {
            let raw = iri_node.text().to_string();
            shape_iris.push(SmolStr::from(raw.trim()));
        }
    }

    // Source binding: scan tokens after KW_FROM for a single IDENT. Complex
    // expressions are signalled as `None`; Phase 3 handles them via the body
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
    // The Phase 1 hello.fossil parser wraps the from-source in `EXPR >
    // LITERAL_EXPR > IDENT` — peek into that shape if the direct-token scan
    // above didn't find one.
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
        shape_iris,
        source_binding,
        body_property_count,
    })
}

fn extract_import_header(node: &SyntaxNode, ast_id: FileAstId<ImportNode>) -> Option<ImportHeader> {
    // Path text — concatenated text of the IMPORT_PATH subnode (preserves
    // segments + slashes verbatim; Phase 3 splits if needed).
    let path_node = node
        .children()
        .find(|c| c.kind() == SyntaxKind::IMPORT_PATH)?;
    let path: String = path_node.text().to_string();
    // Alias: first IDENT after KW_AS at this node's token sequence.
    let mut alias = None;
    let mut after_as = false;
    for tok in node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
    {
        if tok.kind() == SyntaxKind::KW_AS {
            after_as = true;
            continue;
        }
        if after_as && tok.kind() == SyntaxKind::IDENT {
            alias = Some(SmolStr::from(tok.text()));
            break;
        }
    }
    Some(ImportHeader {
        ast_id,
        path: SmolStr::from(path.trim()),
        alias,
    })
}

fn extract_definition_header(
    node: &SyntaxNode,
    ast_id: FileAstId<DefinitionNode>,
) -> Option<DefinitionHeader> {
    let name = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    Some(DefinitionHeader {
        ast_id,
        name: SmolStr::from(name.text()),
    })
}

fn extract_exported_definition_header(
    node: &SyntaxNode,
    ast_id: FileAstId<DefinitionNode>,
) -> Option<ExportedDefinitionHeader> {
    let inner = extract_definition_header(node, ast_id)?;
    let has_type_annotation = node
        .children()
        .any(|c| c.kind() == SyntaxKind::TYPE_ANNOTATION);
    Some(ExportedDefinitionHeader {
        inner,
        has_type_annotation,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

    #[test]
    fn item_tree_for_hello_fossil_has_three_items_signatures_only() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, HELLO_FOSSIL.to_string(), "hello.fossil".to_string());
        let it = item_tree(&db, file);
        let items = it.items(&db);
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0], ItemHeader::PrefixDecl(_)));
        assert!(matches!(items[1], ItemHeader::SourceDef(_)));
        match &items[2] {
            ItemHeader::Mapping(h) => {
                assert_eq!(h.name.as_str(), "User");
                assert_eq!(h.body_property_count, 2);
                assert_eq!(h.source_binding.as_deref(), Some("users"));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    /// Compile-time + runtime guarantee that `ItemHeader::Mapping` does not
    /// carry property values. Editing a property's right-hand side (`.a` →
    /// `.b`) leaves the structural signal (count, names, shape) unchanged.
    #[test]
    fn item_tree_excludes_body() {
        let src_a = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    ex:a = .a
";
        let src_b = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    ex:a = .b
"; // .a → .b — body-only edit
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db_a = fossil_base::FossilDb::new(system.clone());
        let db_b = fossil_base::FossilDb::new(system);
        let f_a = fossil_base::SourceFile::new(&db_a, src_a.to_string(), "x.fossil".to_string());
        let f_b = fossil_base::SourceFile::new(&db_b, src_b.to_string(), "x.fossil".to_string());
        let it_a = item_tree(&db_a, f_a);
        let it_b = item_tree(&db_b, f_b);
        let items_a = it_a.items(&db_a);
        let items_b = it_b.items(&db_b);
        assert_eq!(items_a.len(), items_b.len());
        let m_a = match &items_a[2] {
            ItemHeader::Mapping(m) => m,
            other => panic!("expected Mapping in items_a[2], got {other:?}"),
        };
        let m_b = match &items_b[2] {
            ItemHeader::Mapping(m) => m,
            other => panic!("expected Mapping in items_b[2], got {other:?}"),
        };
        assert_eq!(m_a.name, m_b.name);
        assert_eq!(m_a.shape_iris, m_b.shape_iris);
        assert_eq!(m_a.body_property_count, m_b.body_property_count);
        assert_eq!(m_a.source_binding, m_b.source_binding);
    }
}
