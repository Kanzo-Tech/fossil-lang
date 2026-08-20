//! Typed AST views over the lossless CST.
//!
//! Only the wrappers downstream `fossil-hir` needs to walk the program:
//! `SourceDef`, `Mapping`, `MappingHeader`, `MappingBody`, `Property`. Each is
//! a thin newtype around `SyntaxNode` with `cast` (kind-checking constructor) +
//! `syntax` (back-edge accessor) and a few convenience accessors for child
//! tokens. A view is added when a consumer needs it, never ahead of one.
//!
//! All AST nodes deliberately keep the underlying [`SyntaxNode`] public via
//! `syntax()` so consumers can drop down to the lossless tree when needed —
//! offsets, trivia and error recovery are only visible there.

pub mod items;
pub use items::ShapeExpr;

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

// `PrefixDecl` was a view here, with `name()` and `iri()` accessors reading a
// `PREFIX_DECL`'s `IDENT` and `ABS_IRI` tokens. The node kind is gone and so are
// both tokens — there is no vocabulary declaration left to introduce, and a
// constant IRI is a STRING; its two consumers were
// `fossil_hir::def_map`'s prefix table and `fossil_ide::prefix_index`, and both
// went with it.
ast_node!(SourceDef, SOURCE_DEF);
ast_node!(Mapping, MAPPING);
ast_node!(MappingHeader, MAPPING_HEADER);
ast_node!(MappingBody, MAPPING_BODY);
ast_node!(Property, PROPERTY);

impl SourceDef {
    /// The bound name on the LHS of `:=` (e.g. `users` in `users := io.csv(...)`).
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        // The first IDENT child token is the binding name; the call expression
        // contributes its own IDENT tokens nested inside its EXPR subtree.
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| smol_str::SmolStr::from(t.text()))
    }
}

impl Mapping {
    #[must_use]
    pub fn header(&self) -> Option<MappingHeader> {
        self.0.children().find_map(MappingHeader::cast)
    }

    #[must_use]
    pub fn body(&self) -> Option<MappingBody> {
        self.0.children().find_map(MappingBody::cast)
    }
}

impl MappingBody {
    pub fn properties(&self) -> impl Iterator<Item = Property> {
        self.0.children().filter_map(Property::cast)
    }
}

impl MappingHeader {
    /// The header's mapping name (the `IDENT` immediately before the
    /// `SHAPE_SEP` — `User` in `User : ex:Person from users`).
    #[must_use]
    pub fn name(&self) -> Option<smol_str::SmolStr> {
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| smol_str::SmolStr::from(t.text()))
    }

    /// The header's `ShapeExpr` (`Person` in `Users : Person from Adults`).
    /// `fossil_hir`'s `ItemTree` consumes this to build `Mapping.shape_id`.
    #[must_use]
    pub fn shape_expr(&self) -> Option<items::ShapeExpr> {
        self.0.children().find_map(items::ShapeExpr::cast)
    }

    /// The expression on the right of `from` — the source-binding
    /// reference (`users` in `examples/hello.fossil`). Returned as a
    /// raw [`SyntaxNode`] kept at `EXPR` kind because the upstream Pratt
    /// parser already wraps every right-hand side in an EXPR composite.
    #[must_use]
    pub fn source_expr(&self) -> Option<SyntaxNode> {
        self.0.children().find(|c| c.kind() == SyntaxKind::EXPR)
    }
}
