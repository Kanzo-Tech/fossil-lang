//! What the name under the cursor names — the one place a feature turns a
//! token into a definition.
//!
//! Two halves, and only the first is this crate's. The tree says which TABLE a
//! name is read from — a header's shape and a field's type are types, a
//! binding's own name and the head of an expression are values, a header's
//! first name is a mapping's — and [`fossil_hir::def_map::DefMap::resolve`]
//! answers which definition that is. There is no second resolver: the def map
//! is what lowering and the checker read, so a name the editor resolves is the
//! name the program means.
//!
//! The shape is rust-analyzer's `NameClass`/`NameRefClass` over `hir`, and
//! `ty_ide`'s goto over `ty_python_semantic`'s `SemanticModel`: classify the
//! node, then ask the semantic layer. A new kind of definition is a variant of
//! [`Definition`] and a row of the match below; goto-definition and the outline
//! read it without a branch of their own.

use fossil_base::SourceFile;
use fossil_hir::def_map::{Definition, MappingLoc, Namespace, def_map, mapping_of};
use fossil_syntax::{SyntaxKind, SyntaxToken};
use smol_str::SmolStr;

/// What an identifier names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameAt<'db> {
    /// A name the program binds at the top level — its definition, or a use
    /// of it.
    Definition(Definition<'db>),
    /// A property key: a field of the type the mapping targets. Not a
    /// top-level name, so the mapping is what scopes it.
    Key {
        /// The mapping whose type declares the key.
        mapping: MappingLoc<'db>,
        /// The key as written.
        name: SmolStr,
    },
}

/// Classify `token` by its place in the tree and resolve it.
///
/// `None` for every token that is not a name the program binds: a member
/// after a `.` (a column or a catalogue row, whose definition is not in the
/// program), a named argument, a join alias, the document's own name in
/// `Place as City`, and every name the def map does not bind.
pub fn name_at<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    token: &SyntaxToken,
) -> Option<NameAt<'db>> {
    use SyntaxKind as K;
    if token.kind() != K::IDENT {
        return None;
    }
    let parent = token.parent()?;
    let dm = def_map(db, file);
    let resolve = |namespace| {
        dm.resolve(db, namespace, token.text())
            .map(NameAt::Definition)
    };
    match parent.kind() {
        K::SOURCE_DEF | K::MAPPING_HEADER if !is_first_ident(token) => None,
        K::SOURCE_DEF => resolve(Namespace::Value),
        K::MAPPING_HEADER => resolve(Namespace::Mapping),
        K::IMPORT_MEMBER => {
            let member = fossil_syntax::ast::ImportMember::cast(parent.clone())?;
            if member.name_token().as_ref() != Some(token) {
                return None;
            }
            match parent.parent()?.kind() {
                K::MULTI_SOURCE_DEF => resolve(Namespace::Value),
                _ => resolve(Namespace::Type),
            }
        }
        K::TYPE_DECL => {
            let decl = fossil_syntax::ast::TypeDecl::cast(parent)?;
            (decl.name_token().as_ref() == Some(token))
                .then(|| resolve(Namespace::Type))
                .flatten()
        }
        K::SHAPE_EXPR | K::TYPE_EXPR => resolve(Namespace::Type),
        // `PropertyLhs := IDENT`; `@subject` lexes as `AT_ATTR`, so it never
        // reaches this arm.
        K::PROPERTY_LHS => {
            let node = token.parent_ancestors().find(|n| n.kind() == K::MAPPING)?;
            Some(NameAt::Key {
                mapping: mapping_of(db, file, &node)?,
                name: SmolStr::from(token.text()),
            })
        }
        // A bare name in an expression is a value, or a type written as a call
        // — `Person(User.email)` is an edge to that type.
        K::LITERAL_EXPR => resolve(Namespace::Value).or_else(|| resolve(Namespace::Type)),
        _ => None,
    }
}

/// Whether `token` is the first `IDENT` of its parent — the bound name of a
/// `SOURCE_DEF` and a `MAPPING_HEADER`, which both start with it.
fn is_first_ident(token: &SyntaxToken) -> bool {
    std::iter::successors(
        token.prev_sibling_or_token(),
        fossil_syntax::SyntaxElement::prev_sibling_or_token,
    )
    .all(|el| el.kind() != SyntaxKind::IDENT)
}
