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

// `IriExpr` was a view here, over the `IRI_EXPR` node that wrapped the CURIE,
// the `<…>` absolute IRI and the backtick template. All three spellings went —
// a constant IRI is a STRING now — and so did the node.

impl ShapeExpr {
    /// The shape's NAME — `Person` in `Users : Person from Adults`.
    ///
    /// `ShapeExpr := IDENT` (grammar.bnf, ShapeExpr): one of the names a
    /// `type { … } := io.shex(…)` binding introduced. There is exactly one, and
    /// there was exactly one before: the `&` intersection went when the
    /// lowering was found to keep the first element and drop the rest without a
    /// diagnostic. What changed is that the one is a bare name rather than an
    /// `IRI_EXPR` that could be a CURIE, an absolute IRI or a template — so
    /// this accessor returns the name, and there is no second node to unwrap.
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| smol_str::SmolStr::from(t.text()))
    }
}
