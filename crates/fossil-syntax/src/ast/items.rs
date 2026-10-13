//! Typed AST views for the composite item nodes.
//!
//! Each `ast_node!` here is a thin newtype around a [`SyntaxNode`] with a
//! kind-checking `cast` constructor and a `syntax` back-edge to the
//! underlying lossless node. Downstream consumers — `fossil_hir`'s
//! `body(mapping)` query and the HIR lowering in `fossil-hir::lower` —
//! cast top-level CST children into these views to walk the structural
//! sub-nodes (`MappingHeader.shape_expr()`, `Mapping.body()`, etc.).
//!
//! `super` (`crates/fossil-syntax/src/ast/mod.rs`) holds the rest of the views;
//! the item views added alongside the full item parser live here.

use crate::kind::{SyntaxKind, SyntaxNode};

macro_rules! ast_node {
    ($name:ident, $kind:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(SyntaxNode);

        impl $name {
            #[must_use]
            pub fn cast(node: SyntaxNode) -> Option<Self> {
                if node.kind() == SyntaxKind::$kind {
                    Some(Self(node))
                } else {
                    None
                }
            }

            #[must_use]
            pub const fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

ast_node!(ShapeExpr, SHAPE_EXPR);

impl ShapeExpr {
    /// The shape's NAME — `Person` in `Users : Person from Adults`.
    ///
    /// `ShapeExpr := IDENT` (grammar.bnf, ShapeExpr): one of the names a
    /// `type { … } := io.shex(…)` binding introduced. There is exactly one, and
    /// it is a bare name — so this accessor returns the name, and there is no
    /// second node to unwrap.
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| smol_str::SmolStr::from(t.text()))
    }
}

ast_node!(ImportMember, IMPORT_MEMBER);
ast_node!(TypeDecl, TYPE_DECL);
ast_node!(Field, FIELD);
ast_node!(IriAttr, IRI_ATTR);
ast_node!(TypeExpr, TYPE_EXPR);

/// The `IDENT` tokens directly under `node`, trivia and nested nodes skipped.
fn idents(node: &SyntaxNode) -> impl Iterator<Item = crate::SyntaxToken> {
    node.children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::IDENT)
}

impl ImportMember {
    /// The local name of the shape in the document — `Place` in
    /// `Place as City`.
    #[must_use]
    pub fn shape_name(&self) -> Option<smol_str::SmolStr> {
        idents(&self.0)
            .next()
            .map(|t| smol_str::SmolStr::from(t.text()))
    }

    /// The name the program uses: the one after `as` when the member has one,
    /// else [`Self::shape_name`]. An `as` with nothing after it is the parser's
    /// to refuse, and the member keeps its own name.
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        self.name_token().map(|t| smol_str::SmolStr::from(t.text()))
    }

    /// The token [`Self::name`] reads, for a caller that needs where it is.
    #[must_use]
    pub fn name_token(&self) -> Option<crate::SyntaxToken> {
        let all: Vec<_> = idents(&self.0).collect();
        match all.as_slice() {
            [_, _, local] => Some(local.clone()),
            [name, ..] => Some(name.clone()),
            [] => None,
        }
    }
}

impl TypeDecl {
    /// The `@iri("…")` above the declaration, when it was written.
    #[must_use]
    pub fn iri_attr(&self) -> Option<IriAttr> {
        self.0.children().find_map(IriAttr::cast)
    }

    /// The declared name — `Person` in `type Person`: the `IDENT` after the
    /// contextual `type`.
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        idents(&self.0)
            .skip_while(|t| t.text() != "type")
            .nth(1)
            .map(|t| smol_str::SmolStr::from(t.text()))
    }

    pub fn fields(&self) -> impl Iterator<Item = Field> {
        self.0.children().filter_map(Field::cast)
    }
}

impl Field {
    /// The `@iri("…")` the field starts with, when it was written.
    #[must_use]
    pub fn iri_attr(&self) -> Option<IriAttr> {
        self.0.children().find_map(IriAttr::cast)
    }

    /// The field's name — what a mapping body writes as a property key.
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        idents(&self.0)
            .next()
            .map(|t| smol_str::SmolStr::from(t.text()))
    }

    #[must_use]
    pub fn type_expr(&self) -> Option<TypeExpr> {
        self.0.children().find_map(TypeExpr::cast)
    }
}

impl IriAttr {
    /// The IRI, unquoted. `None` when the attribute has no string.
    ///
    /// The quotes are stripped and nothing else is: `IriAttr` takes a `STRING`
    /// token, which has no hole, and an IRI has no escape worth decoding.
    #[must_use]
    pub fn iri(&self) -> Option<smol_str::SmolStr> {
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| t.kind() == SyntaxKind::STRING)
            .map(|t| {
                let text = t.text();
                let inner = text
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(text);
                smol_str::SmolStr::from(inner)
            })
    }
}

impl TypeExpr {
    /// The constructor or the type — `Option` in `Option<Date>`, `Date` in
    /// `Date`.
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        idents(&self.0)
            .next()
            .map(|t| smol_str::SmolStr::from(t.text()))
    }

    /// The argument between `<` and `>`, when there is one.
    #[must_use]
    pub fn argument(&self) -> Option<Self> {
        self.0.children().find_map(Self::cast)
    }
}
