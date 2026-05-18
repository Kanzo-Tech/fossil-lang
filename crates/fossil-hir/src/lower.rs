//! CST → HIR lowering.
//!
//! Phase 1 ships the lowering for the canonical `examples/hello.fossil`: one
//! [`HirMapping`] per `MAPPING` CST node, with header-level fields (name,
//! resolved shape IRI, source binding) and a flat list of [`HirProperty`]
//! values whose right-hand side is a [`HirExpr`].
//!
//! The lowered shape is what `fossil-mir` (Plan 04) consumes to build the
//! typed operator algebra. The expression encoding is intentionally minimal:
//! - [`HirExpr::Template`] keeps the raw backtick text including `${...}`
//!   placeholders. Codegen parses the template at SQL-emission time. Phase 4
//!   lifts template parsing into a real expression tree.
//! - [`HirExpr::FieldRef`] is just the field name (`.id` → `"id"`).
//! - [`HirExpr::PrefixedName`] carries the already-resolved full IRI.
//! - [`HirExpr::StringLit`] holds the literal text without surrounding quotes.

use fossil_base::SourceFile;
use smol_str::SmolStr;

use crate::def_map::{PrefixEntry, def_map};

#[salsa::tracked(debug)]
pub struct HirFile<'db> {
    #[returns(ref)]
    pub mappings: Vec<HirMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirMapping {
    /// Mapping name, e.g. `"User"`.
    pub name: SmolStr,
    /// Fully-resolved shape IRI, e.g. `"https://example.org/Person"`.
    pub shape_iri: SmolStr,
    /// Name of the source binding referenced by `from`, e.g. `"users"`.
    pub source_binding: SmolStr,
    pub properties: Vec<HirProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirProperty {
    pub key: PropertyKey,
    pub value: HirExpr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PropertyKey {
    /// `iri = ...` — the special "subject IRI" property of a mapping.
    Iri,
    /// `ex:name = ...` — predicate IRI built from `prefix:local` resolved
    /// against the per-file [`crate::DefMap`] prefix table.
    PrefixedName { iri: SmolStr },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum HirExpr {
    /// Backtick template, raw text including delimiters and `${...}`
    /// placeholders. Phase 4 lifts template parsing into a real expression
    /// tree; Phase 1 codegen parses the template at SQL-emission time.
    Template(SmolStr),
    /// `.id` → field name `"id"`.
    FieldRef(SmolStr),
    /// `"hello"` → literal text without surrounding quotes.
    StringLit(SmolStr),
    /// `ex:foo` resolved to its full IRI.
    PrefixedName { iri: SmolStr },
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn lower_to_hir<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> HirFile<'db> {
    let cst = fossil_syntax::parse(db, file);
    let dm = def_map(db, file);
    let prefixes = dm.prefixes(db);

    let mut mappings = Vec::new();
    for child in cst.root(db).syntax().children() {
        if child.kind() == fossil_syntax::SyntaxKind::MAPPING
            && let Some(m) = lower_mapping_node(&child, prefixes)
        {
            mappings.push(m);
        }
    }
    HirFile::new(db, mappings)
}

fn lookup_prefix(prefixes: &[PrefixEntry], name: &str) -> Option<SmolStr> {
    prefixes
        .iter()
        .find(|e| e.name.as_str() == name)
        .map(|e| e.iri.clone())
}

fn lower_mapping_node(
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirMapping> {
    use fossil_syntax::SyntaxKind;

    let header = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)?;
    let body = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)?;

    // Phase 2 plan 02-03 wraps the header's shape and source in composite
    // sub-nodes:
    //
    //   MAPPING_HEADER
    //     IDENT "User"                      -- direct token: mapping name
    //     SHAPE_SEP ":"
    //     SHAPE_EXPR
    //       IRI_EXPR
    //         IDENT "ex" SHAPE_SEP ":" IDENT "Person"
    //     (optional) IN_CLAUSE
    //     KW_FROM "from"
    //     EXPR
    //       LITERAL_EXPR
    //         IDENT "users"                 -- the from-source expression
    //
    // Phase 1's flat "first four IDENTs" shortcut no longer matches; walk the
    // sub-nodes by kind to extract each header field cleanly.
    let name = header
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))?;

    // ShapeExpr → first IRI_EXPR → its (prefix-name, local-name) tokens. For
    // the Phase 1 hello.fossil + Wave 1 fixtures this is the lexer-contiguous
    // `IDENT SHAPE_SEP IDENT` shape; degenerate single-IDENT IRIExprs (from
    // the recovery path) cause us to bail with `None` (Phase 3 promotes to
    // a real diagnostic).
    let shape_expr = header
        .children()
        .find(|c| c.kind() == SyntaxKind::SHAPE_EXPR)?;
    let first_iri = shape_expr
        .children()
        .find(|c| c.kind() == SyntaxKind::IRI_EXPR)?;
    let iri_idents: Vec<_> = first_iri
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .collect();
    if iri_idents.len() < 2 {
        return None;
    }
    let shape_prefix_name = iri_idents[0].text();
    let shape_local = iri_idents[1].text();
    let shape_prefix_iri = lookup_prefix(prefixes, shape_prefix_name)?;
    let shape_iri = SmolStr::from(format!("{shape_prefix_iri}{shape_local}"));

    // Source binding: the EXPR after `from`. For Phase 1's hello.fossil the
    // expression is a bare IDENT primary (`users`), surfacing as
    // `EXPR > LITERAL_EXPR > IDENT`. Recover from the EXPR's first IDENT
    // descendant; falls back to the legacy direct-IDENT shape so any other
    // header form keeps working.
    let source_binding = header
        .children()
        .find(|c| c.kind() == SyntaxKind::EXPR)
        .and_then(|expr_node| {
            expr_node
                .descendants_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::IDENT)
        })
        .map(|t| SmolStr::from(t.text()))?;

    let mut properties = Vec::new();
    for prop_node in body.children().filter(|c| c.kind() == SyntaxKind::PROPERTY) {
        if let Some(p) = lower_property(&prop_node, prefixes) {
            properties.push(p);
        }
    }

    Some(HirMapping {
        name,
        shape_iri,
        source_binding,
        properties,
    })
}

fn lower_property(
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirProperty> {
    use fossil_syntax::SyntaxKind;

    // PropertyLhs: per Phase 2 grammar.bnf line 141, `PropertyLhs := 'iri' | IRIExpr`.
    // The KW_IRI literal is a direct token child of PROPERTY_LHS; a prefixed
    // name is wrapped in an `IRI_EXPR` sub-node (parser plan 02-03). Walk both
    // forms by collecting all IDENT-or-KW_IRI tokens from descendants.
    let lhs_node = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_LHS)?;
    // Use `descendants_with_tokens` so IRI_EXPR-wrapped IDENT/SHAPE_SEP tokens
    // are still discoverable. Skip trivia.
    let lhs_toks: Vec<_> = lhs_node
        .descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect();

    let key = if lhs_toks.len() == 1
        && (lhs_toks[0].kind() == SyntaxKind::KW_IRI || lhs_toks[0].text() == "iri")
    {
        PropertyKey::Iri
    } else if lhs_toks.len() == 3 && lhs_toks[1].kind() == SyntaxKind::SHAPE_SEP {
        let prefix = lhs_toks[0].text();
        let local = lhs_toks[2].text();
        let prefix_iri = lookup_prefix(prefixes, prefix)?;
        PropertyKey::PrefixedName {
            iri: SmolStr::from(format!("{prefix_iri}{local}")),
        }
    } else {
        return None;
    };

    let expr_node = node.children().find(|c| c.kind() == SyntaxKind::EXPR)?;
    let value = lower_expr(&expr_node, prefixes)?;

    Some(HirProperty { key, value })
}

/// Lower an `EXPR` composite node.
///
/// The Phase 1 parser wraps every right-hand side in an `EXPR` whose single
/// child is one of: `TEMPLATE_EXPR`, `LITERAL_EXPR`, `IRI_EXPR`,
/// `FIELD_REF_EXPR`. We dispatch on that inner kind.
fn lower_expr(expr_node: &fossil_syntax::SyntaxNode, prefixes: &[PrefixEntry]) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let inner = expr_node.children().next()?;
    match inner.kind() {
        SyntaxKind::TEMPLATE_EXPR => {
            let tok = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::TEMPLATE)?;
            Some(HirExpr::Template(SmolStr::from(tok.text())))
        }
        SyntaxKind::FIELD_REF_EXPR => {
            // `DOT IDENT` — capture the IDENT.
            let ident = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::IDENT)?;
            Some(HirExpr::FieldRef(SmolStr::from(ident.text())))
        }
        SyntaxKind::LITERAL_EXPR => {
            // Either a `STRING` literal or a bare/prefixed name.
            let toks: Vec<_> = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .filter(|t| {
                    !matches!(
                        t.kind(),
                        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
                    )
                })
                .collect();
            if let Some(s) = toks.iter().find(|t| t.kind() == SyntaxKind::STRING) {
                let raw = s.text();
                let inner_text = raw.trim_start_matches('"').trim_end_matches('"');
                return Some(HirExpr::StringLit(SmolStr::from(inner_text)));
            }
            // `IDENT (SHAPE_SEP IDENT)?` — a bare or prefixed name.
            let idents: Vec<_> = toks
                .iter()
                .filter(|t| t.kind() == SyntaxKind::IDENT)
                .collect();
            if idents.len() == 2 {
                let prefix = idents[0].text();
                let local = idents[1].text();
                let prefix_iri = lookup_prefix(prefixes, prefix)?;
                Some(HirExpr::PrefixedName {
                    iri: SmolStr::from(format!("{prefix_iri}{local}")),
                })
            } else if idents.len() == 1 {
                // Bare identifier — Phase 1 has no other meaning for this so
                // surface it as a FieldRef-shaped expression. Phase 2 grammar
                // distinguishes more precisely (variable vs field vs name).
                Some(HirExpr::FieldRef(SmolStr::from(idents[0].text())))
            } else {
                None
            }
        }
        SyntaxKind::IRI_EXPR => {
            let abs = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::ABS_IRI)?;
            let iri = abs.text().trim_start_matches('<').trim_end_matches('>');
            Some(HirExpr::PrefixedName {
                iri: SmolStr::from(iri),
            })
        }
        _ => None,
    }
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

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    #[test]
    fn lower_hello_produces_one_mapping_with_two_properties() {
        let (db, file) = db_with_hello();
        let hir = lower_to_hir(&db, file);
        let mappings = hir.mappings(&db);
        assert_eq!(mappings.len(), 1);
        let m = &mappings[0];
        assert_eq!(m.name.as_str(), "User");
        assert_eq!(m.shape_iri.as_str(), "https://example.org/Person");
        assert_eq!(m.source_binding.as_str(), "users");
        assert_eq!(m.properties.len(), 2);
    }

    #[test]
    fn lower_hello_property_zero_is_iri_template() {
        let (db, file) = db_with_hello();
        let hir = lower_to_hir(&db, file);
        let mappings = hir.mappings(&db);
        let p0 = &mappings[0].properties[0];
        assert!(matches!(p0.key, PropertyKey::Iri));
        match &p0.value {
            HirExpr::Template(t) => assert!(t.contains("${.id}"), "template text was {t:?}"),
            other => panic!("expected Template, got {other:?}"),
        }
    }

    #[test]
    fn lower_hello_property_one_is_prefixed_name_field_ref() {
        let (db, file) = db_with_hello();
        let hir = lower_to_hir(&db, file);
        let mappings = hir.mappings(&db);
        let p1 = &mappings[0].properties[1];
        match &p1.key {
            PropertyKey::PrefixedName { iri } => {
                assert_eq!(iri.as_str(), "https://example.org/name");
            }
            PropertyKey::Iri => panic!("expected PrefixedName key, got Iri"),
        }
        match &p1.value {
            HirExpr::FieldRef(f) => assert_eq!(f.as_str(), "name"),
            other => panic!("expected FieldRef value, got {other:?}"),
        }
    }

    #[test]
    fn lower_to_hir_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let a = lower_to_hir(&db, file);
        let b = lower_to_hir(&db, file);
        assert_eq!(a, b);
    }
}
