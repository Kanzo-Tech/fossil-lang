//! `textDocument/definition` — goto-def across the open-file set **and across
//! the language boundary into the shape document**.
//!
//! # The boundary this crosses is a LANGUAGE, not a prefix
//!
//! Goto-def used to be pure name resolution inside the open-file set: a token,
//! a [`crate::WorkspaceIndex`] lookup, a byte range. That worked because the
//! two things worth jumping to — a `prefix` declaration and a mapping header —
//! were both `.fossil`. `prefix` is gone — it is an ordinary identifier now, and
//! there is no vocabulary declaration left to introduce — and with it the
//! only reason a Fossil program ever referred to a name in another Fossil file.
//!
//! What replaced it is bigger. Ruling 3 of 2026-08-11 makes naming a shape
//! document MANDATORY, and a property key is the **last segment
//! of a predicate IRI the document declares**. So the two names a body writes
//! most are both defined in a file that is not a Fossil program:
//!
//! - a **shape name** in a header — `Users : Person from Adults` — is one of
//!   the names `type { Person } := io.shex("shop.shex")` introduced, and what
//!   it denotes is a shape inside `shop.shex`;
//! - a **property key** — `name = User.name` — denotes a predicate inside that
//!   same document.
//!
//! Neither has a definition anywhere in the `.fossil` file. Answering "no
//! definition" for the two commonest identifiers in a program is worse than
//! answering imprecisely, which is what the rest of this module is about.
//!
//! # Precision: the decoder's own spans
//!
//! The decoded shape carries where the document declares it and each of its
//! predicates — [`fossil_graph_schema::Shape::span`] and
//! [`fossil_graph_schema::PropertyConstraint::span`], found by
//! `fossil_shex::spans` with rudof's own prefix spellings. When there is no span
//! (`ShExJ`, SHACL), the answer is still the right FILE with a `0..0` range —
//! the top of the document. That is the honest degraded answer, and it is
//! deliberately not a `None`: an editor that opens `shop.shex` at line 1 has
//! taken the user to the definition's file, which is most of the value;
//! answering nothing takes them nowhere.
//!
//! [`NavigationTarget`] carries no precision flag, because no consumer could
//! act on one — `fossil-lsp` and `fossil-wasm` both translate `{file, range}`
//! into an `lsp_types::Location` and nothing else. `0..0` IS the flag, and it
//! is one every consumer already understands.
//!
//! # Domain boundary
//!
//! Returns Fossil-domain [`NavigationTarget`]s (`{ file, range: Range<u32> }`),
//! NEVER `lsp_types::Location` — the byte→UTF-16 range translation happens in
//! `fossil-lsp` via [`crate::position::position`], so this stays
//! transport-free and WASM-clean. No new Salsa query is added: the
//! `WorkspaceIndex` is a plain struct built by a CST walk, and the two document
//! paths read queries that the checker already runs for this file
//! (`resolve_target_shape`, `typecheck_mapping`), so the per-mapping `body()`
//! fan-out is unchanged.
//!
//! # Nothing here does index arithmetic
//!
//! There were three notions of `ExprId` in the tree and one of them was a
//! position among the CST's `PROPERTY` children used to index the HIR's dense
//! `properties` vector. This module never needs one: a property key is resolved
//! by its TEXT against the checker's `predicates` table, so a property that
//! parses and does not lower shifts nothing. The only index taken is the
//! mapping's, which is the per-kind dense index `def_map` and `body()` both
//! contract to, and it is taken by the same filter-then-position
//! walk they use.

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::shapes::resolve_target_shape;
use fossil_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use crate::WorkspaceIndex;
use crate::position::token_at_position;
use fossil_hir::documents::registry_key;

/// A goto-def navigation target: the owning file + the byte range.
///
/// `fossil-lsp` translates `range` to a UTF-16 `lsp_types::Range` and pairs it
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
/// Three positions are recognised, and the first two leave the `.fossil` file:
///
/// 1. **a shape name** in a mapping header → the shape inside the document its
///    `type { … } := io.shex("…")` binding names, or — when the document is not
///    registered — that binding line;
/// 2. **a property key** → the predicate inside the same document;
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

/// Resolve a name that lives in the shape document, and answer with a place in
/// that document.
///
/// The shape is the mapping's own target, [`resolve_target_shape`] — the
/// document that declared it, not the first one the file names — and a
/// property key resolves through the checker's `predicates` table, the same
/// one hover and `fossil-mir` read, so a renamed predicate lands where the
/// checker says it is.
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
    let Some(doc_file) = fossil_base::file_at(db, &registry_key(db, file, &shape.document)) else {
        return Vec::new();
    };
    let span = match wanted {
        Wanted::Shape(_) => shape.span,
        Wanted::Predicate(key) => {
            let Some(iri) = typecheck_mapping(db, mapping).ok().and_then(|out| {
                out.predicates(db)
                    .iter()
                    .find(|(short, _)| short.as_str() == key.as_str())
                    .map(|(_, iri)| iri.clone())
            }) else {
                return Vec::new();
            };
            shape.constraint_for(&iri).and_then(|c| c.span)
        }
    };
    vec![NavigationTarget {
        file: doc_file,
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

/// The names between `{` and `}` of a `TYPE_DEF` — `Person`, `Order` in
/// `type { Person, Order } := io.shex("shop.shex")`.
///
/// The IDENTs after the closing brace belong to the constructor (`io`, `shex`),
/// so the scan stops there.
fn brace_members(node: &SyntaxNode) -> Vec<String> {
    let mut members = Vec::new();
    let mut inside = false;
    for token in node
        .descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
    {
        match token.kind() {
            SyntaxKind::LBRACE => inside = true,
            SyntaxKind::RBRACE => break,
            SyntaxKind::IDENT if inside => members.push(token.text().to_string()),
            _ => {}
        }
    }
    members
}
