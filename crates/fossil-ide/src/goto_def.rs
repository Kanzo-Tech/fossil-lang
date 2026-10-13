//! `textDocument/definition` — where a name a program writes is declared.
//!
//! The two names a mapping body writes most are a type's: the **type name** in
//! a header (`Users : Person from Adults`) and a **property key**
//! (`name = User.name`). Both are declared where the type is — in the program
//! for a declared type, in the shape document for an imported one — and the
//! answer comes from the mapping's resolved type, which knows its own file
//! ([`fossil_hir::shapes::ResolvedShape::file`]). This module never asks which
//! kind of type it is looking at. Every other name goes through the
//! [`crate::WorkspaceIndex`] over the open files.
//!
//! # Precision
//!
//! A declaration carries its span: the type's name and each field's, or the
//! decoder's own spans in a document. Where a decoder has none (`ShExJ`,
//! SHACL), the answer is the right file with a `0..0` range — the top of the
//! document, which takes the reader to the definition's file rather than
//! nowhere. `0..0` is the precision flag, and every consumer already reads it.
//!
//! # Domain boundary
//!
//! Returns Fossil-domain [`NavigationTarget`]s (`{ file, range: Range<u32> }`),
//! never `lsp_types::Location`: the byte→UTF-16 translation is the host's,
//! through [`crate::position::position`], so this stays transport-free and
//! wasm-clean. A property key resolves by its text against the checker's
//! `predicates` table, so nothing here indexes into a body.

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::shapes::resolve_target_shape;
use fossil_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use crate::WorkspaceIndex;
use crate::position::token_at_position;

/// A goto-def navigation target: the owning file + the byte range.
///
/// The host translates `range` to a UTF-16 `lsp_types::Range` and pairs it
/// with the file URI to build a `lsp_types::Location`.
///
/// **`range == 0..0` means "this file, position unknown"** — see the module
/// docs. It is produced only by the two document-crossing paths, and only when
/// the decoder kept no span for what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    /// The file that contains the definition. May be a `.fossil` program or the
    /// shape document a program names.
    pub file: SourceFile,
    /// Byte range of the definition site into that file's text.
    pub range: Range<u32>,
}

/// Resolve the definition(s) of the identifier at an LSP position.
///
/// `files` is the host's open-file set (the LSP `LspState.files` or the
/// wasm workspace's open files); `file` is the file the cursor is in.
///
/// Three positions are recognised, and the first two go where the type is
/// declared — the program for a declared type, the document for an imported one:
///
/// 1. **a shape name** in a mapping header → the type's declaration, or — when
///    the document an import names is not registered — that binding line;
/// 2. **a property key** → the field or predicate that declares it;
/// 3. **anything else** → the [`WorkspaceIndex`] over the open files, which is
///    what resolves a mapping name.
///
/// Returns ALL matching definition sites for case 3 (a name may be declared in
/// more than one open file — the index does not silently drop ambiguity), and
/// an empty `Vec` when the cursor is not on a resolvable identifier.
///
/// `line` / `character` are UTF-16 LSP coordinates (resolved via the
/// [`crate::position::LineIndex`]); never byte offsets.
#[must_use]
pub fn goto_definition(
    db: &dyn fossil_base::Db,
    files: &[SourceFile],
    file: SourceFile,
    line: u32,
    character: u32,
) -> Vec<NavigationTarget> {
    let Some(token) = token_at_position(db, file, line, character) else {
        return Vec::new();
    };

    // The two document-crossing positions answer for themselves, empty
    // included. Falling through to the workspace index would be worse than
    // nothing for both: it records a mapping header's shape name under the
    // header's own range, so a cursor on `Person` would "resolve" to the
    // `Person` it is already sitting on — a jump that goes nowhere and looks
    // like it worked.
    if let Some((mapping, wanted)) = document_name_under_cursor(&token) {
        return document_targets(db, file, &mapping, &wanted);
    }

    // A mapping name: the token under the cursor IS the name.
    let ws = WorkspaceIndex::build(db, files);
    let mut targets = Vec::new();
    for (decl_file, entry) in ws.resolve(token.text()) {
        let target = NavigationTarget {
            file: decl_file,
            range: entry.range,
        };
        // A doubled location is one the LSP would show twice.
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

/// What the cursor asked the document for, by the name it is on.
enum Wanted {
    /// The shape itself — the cursor was on the header's shape name.
    Shape(String),
    /// The predicate a bare property key names.
    Predicate(String),
}

/// The enclosing `MAPPING` and what the cursor asks its document for, when the
/// cursor is on one of the two names a document defines.
///
/// - `ShapeExpr := IDENT` under a `MAPPING_HEADER` (grammar.bnf, `ShapeExpr`).
///   The token's own parent is the `SHAPE_EXPR`; climbing further from a
///   different IDENT would claim the mapping name and the `from` expression,
///   which are ordinary Fossil-side names.
/// - `PropertyLhs := IDENT` (grammar.bnf, `PropertyLhs`). `@subject` is
///   deliberately NOT one: it lexes as `AT_ATTR`, and a shape declares a
///   node's predicates while in RDF the subject IS the node.
fn document_name_under_cursor(token: &SyntaxToken) -> Option<(SyntaxNode, Wanted)> {
    if token.kind() != SyntaxKind::IDENT {
        return None;
    }
    let name = token.text().to_string();
    let wanted = match token.parent()?.kind() {
        SyntaxKind::SHAPE_EXPR => Wanted::Shape(name),
        SyntaxKind::PROPERTY_LHS => Wanted::Predicate(name),
        _ => return None,
    };
    let mapping = token
        .parent_ancestors()
        .find(|n| n.kind() == SyntaxKind::MAPPING)?;
    Some((mapping, wanted))
}

/// Resolve a name the mapping's type declares, and answer with the place it is
/// declared — wherever [`ResolvedShape::file`](fossil_hir::shapes::ResolvedShape::file) says that is.
///
/// The shape is the mapping's own target, [`resolve_target_shape`] — the
/// document that declared it, not the first one the file names — and a
/// property key resolves through the checker's `predicates` table, the same
/// one hover and `fossil-mir` read, so a key lands where the checker says it
/// is.
fn document_targets(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    mapping_node: &SyntaxNode,
    wanted: &Wanted,
) -> Vec<NavigationTarget> {
    let Some(mapping) = fossil_hir::def_map::mapping_of(db, file, mapping_node) else {
        return Vec::new();
    };
    let Some(shape) = resolve_target_shape(db, mapping) else {
        // The document is named and not registered — an unsaved buffer the host
        // has not opened, a path that does not exist, a browser host with no
        // filesystem — or the header names a shape no binding introduced. The
        // binding line is a real definition site for the NAME, so a shape lands
        // there when there is one; a predicate does not, because the binding
        // says nothing about which predicates the document declares.
        return match wanted {
            Wanted::Shape(name) => type_binding_range(db, file, name)
                .map(|range| NavigationTarget { file, range })
                .into_iter()
                .collect(),
            Wanted::Predicate(_) => Vec::new(),
        };
    };
    let Some(home) = shape.file else {
        return Vec::new();
    };
    let span = match wanted {
        Wanted::Shape(_) => shape.span,
        Wanted::Predicate(key) => {
            let Some(iri) = typecheck_mapping(db, mapping)
                .checked(db)
                .ok()
                .and_then(|out| {
                    out.predicates(db)
                        .iter()
                        .find(|p| p.name.as_str() == key.as_str())
                        .map(|p| p.iri.clone())
                })
            else {
                return Vec::new();
            };
            shape.constraint_for(&iri).and_then(|c| c.span)
        }
    };
    vec![NavigationTarget {
        file: home,
        range: span.map_or(0..0, Into::into),
    }]
}

/// The byte range of the `type { … } := io.shex("…")` binding that introduced
/// `name` — the fallback when the document it names is not registered.
fn type_binding_range(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    name: &str,
) -> Option<Range<u32>> {
    let cst = fossil_syntax::parse(db, file);
    cst.root(db)
        .syntax()
        .children()
        .filter(|c| c.kind() == SyntaxKind::TYPE_DEF)
        .find(|node| brace_members(node).iter().any(|m| m == name))
        .map(|node| {
            let r = node.text_range();
            u32::from(r.start())..u32::from(r.end())
        })
}

/// The names a `TYPE_DEF` introduces — `Person`, `Order` in
/// `type { Person, Order } := io.shex("shop.shex")`, `City` in
/// `type { Place as City } := …`.
fn brace_members(node: &SyntaxNode) -> Vec<String> {
    node.children()
        .filter_map(fossil_syntax::ast::ImportMember::cast)
        .filter_map(|m| m.name())
        .map(|n| n.to_string())
        .collect()
}
