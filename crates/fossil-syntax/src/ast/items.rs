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
//! `SourceDef` views live in `super` (`crates/fossil-syntax/src/ast/mod.rs`)
//! and stay there for back-compat with the Phase 1 callers. New plan 02-03
//! views (`Import`, `Definition`, `ExportedDefinition`, `RecordLiteral`,
//! `AnnotationBlock`) live in this file.

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

ast_node!(Import, IMPORT);
ast_node!(Definition, DEFINITION);
ast_node!(ExportedDefinition, EXPORTED_DEFINITION);
ast_node!(RecordLiteral, RECORD_LITERAL);
ast_node!(AnnotationBlock, ANNOTATION_BLOCK);
ast_node!(ShapeExpr, SHAPE_EXPR);
ast_node!(InClause, IN_CLAUSE);
ast_node!(IriExpr, IRI_EXPR);

impl Import {
    /// The first IDENT token in the import — the head segment of the
    /// `use foo/bar` path. Plan 02-04's `ItemTree` will lift this to a
    /// proper `Path::segments()` iterator once it walks `IMPORT_PATH`
    /// composite children.
    #[must_use]
    pub fn head_segment(&self) -> Option<smol_str::SmolStr> {
        self.0
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT || t.kind() == SyntaxKind::STRING)
            .map(|t| smol_str::SmolStr::from(t.text()))
    }
}

impl Definition {
    /// The name on the LHS of `:=` (the IDENT preceding DEFINE).
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| smol_str::SmolStr::from(t.text()))
    }
}

impl ExportedDefinition {
    /// The wrapped `Definition` (the `f := …` after the optional
    /// `TypeAnnotation`). May be `None` in malformed inputs that hit
    /// recovery.
    #[must_use]
    pub fn definition(&self) -> Option<Definition> {
        self.0.children().find_map(Definition::cast)
    }
}

impl ShapeExpr {
    /// The shape's first (and, for the simple `Name : Shape` case, only)
    /// IRI expression. The `&`-intersection alternative shapes follow as
    /// additional [`IriExpr`] siblings.
    #[must_use]
    pub fn primary_iri(&self) -> Option<IriExpr> {
        self.0.children().find_map(IriExpr::cast)
    }
}
