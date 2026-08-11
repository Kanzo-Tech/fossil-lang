//! Typed AST views for the Phase 2 composite item nodes added in plan 02-03.
//!
//! Each `ast_node!` here is a thin newtype around a [`SyntaxNode`] with a
//! kind-checking [`cast`] constructor and a `syntax` back-edge to the
//! underlying lossless node. Downstream consumers (plan 02-04's
//! `ItemTree` / `body(mapping)`; the HIR lowering in `fossil-hir::lower`)
//! cast top-level CST children into these views to walk the structural
//! sub-nodes (`MappingHeader.shape_expr()`, `Mapping.body()`, etc.).
//!
//! The `Mapping`, `MappingHeader`, `MappingBody`, `Property`, `PrefixDecl`,
//! `SourceDef` views live in `super` (`crates/fossil-syntax/src/ast/mod.rs`);
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

// `Import`, `RecordLiteral`, `AnnotationBlock` and `InClause` were views here.
// Each wrapped a node kind the parser built and the HIR never cast, and all
// four node kinds are gone — see `grammar.bnf` for what each one was and what
// it would take to bring it back.

ast_node!(ShapeExpr, SHAPE_EXPR);
ast_node!(IriExpr, IRI_EXPR);

impl ShapeExpr {
    /// The shape's IRI expression. There is exactly one: the `&` intersection
    /// went when the lowering was found to keep the first element and drop the
    /// rest without a diagnostic.
    #[must_use]
    pub fn primary_iri(&self) -> Option<IriExpr> {
        self.0.children().find_map(IriExpr::cast)
    }
}
