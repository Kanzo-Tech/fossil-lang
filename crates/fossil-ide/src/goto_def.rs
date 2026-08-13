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
//! # Precision: the decoder keeps no offsets, and this says so
//!
//! [`fossil_base::shape_document`] answers with a
//! [`fossil_graph_schema::OutputShapes`] — `Shape { iri, properties }` and
//! `PropertyConstraint { predicate, datatype, targets, occurs }`. **There is no
//! span anywhere in that vocabulary**, and there deliberately cannot be a cheap
//! one: salsa memoises the value and decides "did this change?" by `PartialEq`
//! (`fossil_graph_schema::shapes`'s own module docs), so a field that moved
//! whenever a byte of unrelated whitespace moved would invalidate every mapping
//! checked against the document on every edit.
//!
//! So the position inside the document is recovered by a **textual locator**
//! over the document's own bytes ([`locate_iri`]) rather than by the decode,
//! and the locator can fail. When it does, the answer is still the right FILE
//! with a `0..0` range — the top of the document. That is the honest degraded
//! answer, and it is deliberately not a `None`: an editor that opens
//! `shop.shex` at line 1 has taken the user to the definition's file, which is
//! most of the value; answering nothing takes them nowhere.
//!
//! [`NavigationTarget`] carries no precision flag, because no consumer could
//! act on one — `fossil-lsp` and `fossil-wasm` both translate `{file, range}`
//! into an `lsp_types::Location` and nothing else. `0..0` IS the flag, and it
//! is one every consumer already understands.
//!
//! # Domain boundary (Research §fossil-ide split)
//!
//! Returns Fossil-domain [`NavigationTarget`]s (`{ file, range: Range<u32> }`),
//! NEVER `lsp_types::Location` — the byte→UTF-16 range translation happens in
//! `fossil-lsp` via [`crate::position::offset_to_lsp_position`], so this stays
//! transport-free and WASM-clean. No new Salsa query is added: the
//! `WorkspaceIndex` is a plain struct built by a CST walk, and the two document
//! paths read queries that the checker already runs for this file
//! (`def_map`, `resolve_target_shape`), so the per-mapping `body()` fan-out is
//! unchanged (Research Pitfall #3).
//!
//! # Nothing here does index arithmetic
//!
//! There were three notions of `ExprId` in the tree and one of them was a
//! position among the CST's `PROPERTY` children used to index the HIR's dense
//! `properties` vector. This module never needs one: a property key is resolved
//! by its TEXT against the shape's `short_names` table, so a property that
//! parses and does not lower shifts nothing. The only index taken is the
//! mapping's, which is the per-kind dense index `def_map` and `body()` both
//! contract to, and it is taken by the same filter-then-position
//! walk they use.

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_hir::def_map::{MappingLoc, TypeEntry, def_map};
use fossil_hir::shapes::resolve_target_shape;
use fossil_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use crate::WorkspaceIndex;
use crate::position::token_at_position;
use crate::shape_documents::registry_key;

/// A goto-def navigation target: the owning file + the byte range.
///
/// `fossil-lsp` translates `range` to a UTF-16 `lsp_types::Range` and pairs it
/// with the file URI to build a `lsp_types::Location`.
///
/// **`range == 0..0` means "this file, position unknown"** — see the module
/// docs. It is produced only by the two document-crossing paths, and only when
/// the locator could not find the IRI in the document's text.
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
/// playground's multi-panel set); `file` is the file the cursor is in.
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
/// [`crate::line_index::LineIndex`]); never byte offsets.
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
    if let Some(shape) = shape_name_under_cursor(&token) {
        return document_targets(db, file, &shape, &Wanted::Shape);
    }
    if let Some((mapping_node, key)) = property_key_under_cursor(&token) {
        let Some(shape) = header_shape_name(&mapping_node) else {
            return Vec::new();
        };
        let Some(mapping) = mapping_loc(db, file, &mapping_node) else {
            return Vec::new();
        };
        return document_targets(db, file, &shape, &Wanted::Predicate { mapping, key });
    }

    // ONE candidate: the token's own text. A shape used to be `ex:Person` —
    // several leaf tokens under an `IRI_EXPR`, none of which matched the index
    // entry on its own — so a `name_candidates` walk climbed to the enclosing
    // node to recover the full surface text. A shape is a bare `IDENT` now
    // (grammar.bnf, ShapeExpr) and is handled above; what is left here is a
    // mapping name, where the token under the cursor IS the name.
    //
    // A resolution step lived here too: the prefix segment of `ex:Person`,
    // resolved cross-file to the `prefix ex: <…>` line that declared it. There
    // are no prefix declarations, so there is nothing to navigate to.
    let ws = WorkspaceIndex::build(db, files);
    let mut targets = Vec::new();
    for (decl_file, entry) in ws.resolve(token.text()) {
        push_unique(
            &mut targets,
            NavigationTarget {
                file: decl_file,
                range: entry.range,
            },
        );
    }
    targets
}

/// What the cursor asked the document for.
enum Wanted<'db> {
    /// The shape itself — the cursor was on the header's shape name.
    Shape,
    /// The predicate a bare property key names, resolved through the shape's
    /// `short_names` table.
    Predicate {
        mapping: MappingLoc<'db>,
        key: String,
    },
}

/// The shape name the cursor is on, if it is on one.
///
/// `ShapeExpr := IDENT` under a `MAPPING_HEADER` (grammar.bnf, ShapeExpr). The
/// token's own parent is the `SHAPE_EXPR`, so no walk is needed — and no walk is
/// WANTED: climbing further would claim the mapping name and the `from`
/// expression too,
/// which are ordinary Fossil-side names.
fn shape_name_under_cursor(token: &SyntaxToken) -> Option<String> {
    if token.kind() != SyntaxKind::IDENT {
        return None;
    }
    let parent = token.parent()?;
    (parent.kind() == SyntaxKind::SHAPE_EXPR).then(|| token.text().to_string())
}

/// The `(mapping, key)` the cursor is on, if it is on a property key.
///
/// `PropertyLhs := IDENT` (grammar.bnf, PropertyLhs). `@subject` is deliberately
/// NOT one: it lexes as `AT_ATTR`, and a shape declares a node's predicates
/// while in RDF the subject IS the node — there is nothing in the document for
/// it to name.
fn property_key_under_cursor(token: &SyntaxToken) -> Option<(SyntaxNode, String)> {
    if token.kind() != SyntaxKind::IDENT {
        return None;
    }
    let mut node = token.parent()?;
    if node.kind() != SyntaxKind::PROPERTY_LHS {
        return None;
    }
    let key = token.text().to_string();
    loop {
        if node.kind() == SyntaxKind::MAPPING {
            return Some((node, key));
        }
        node = node.parent()?;
    }
}

/// The shape NAME a mapping's header targets.
fn header_shape_name(mapping: &SyntaxNode) -> Option<String> {
    use fossil_syntax::ast::Mapping;
    Mapping::cast(mapping.clone())?
        .header()?
        .shape_expr()?
        .name()
        .map(|n| n.to_string())
}

/// The [`MappingLoc`] for a `MAPPING` node.
///
/// Per plan 02-04 Blocker 2: `MappingLoc.index` is the position among
/// MAPPING-kind top-level children — filter BEFORE indexing. This is the same
/// walk `def_map` and `body()` contract to, and taking it any other way
/// resolves to a different mapping in any file with a `type` binding above it,
/// which is now every file.
fn mapping_loc<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    mapping_node: &SyntaxNode,
) -> Option<MappingLoc<'db>> {
    let cst = fossil_syntax::parse(db, file);
    let index = cst
        .root(db)
        .syntax()
        .children()
        .filter(|c| c.kind() == SyntaxKind::MAPPING)
        .position(|c| &c == mapping_node)?;
    def_map(db, file)
        .mappings(db)
        .iter()
        .find(|m| m.index(db) == index)
        .copied()
}

/// Resolve a name that lives in the shape document, and answer with a place in
/// that document.
///
/// The shape name is resolved against `def_map`'s [`TypeEntry`] table rather
/// than against `DefMap::output_shape_binding`, and the difference is real: the
/// latter takes the FIRST binding that names a document, so in a program with
/// two `type { … }` lines it sends every shape to one file. A name knows which
/// document introduced it.
///
/// [`DefMap::output_shape_binding`]: fossil_hir::def_map::DefMap::output_shape_binding
fn document_targets(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    shape_name: &str,
    wanted: &Wanted<'_>,
) -> Vec<NavigationTarget> {
    let dm = def_map(db, file);
    let Some(entry) = dm.types(db).iter().find(|t| t.name.as_str() == shape_name) else {
        // The header names a shape no `type { … }` binding introduced. That is
        // a diagnostic the checker owns; goto-def has nowhere to go.
        return Vec::new();
    };

    let document = entry.document.as_ref().map(|d| registry_key(db, file, d));
    let doc_file = document.as_deref().and_then(|key| fossil_base::file_at(db, key));

    let Some(doc_file) = doc_file else {
        // The document is named and not registered — an unsaved buffer the host
        // has not opened, a path that does not exist, a playground with no
        // filesystem. The binding line is a real definition site for the NAME,
        // so a shape lands there; a predicate does not, because the binding says
        // nothing about which predicates the document declares.
        return match wanted {
            Wanted::Shape => type_binding_range(db, file, shape_name)
                .map(|range| NavigationTarget { file, range })
                .into_iter()
                .collect(),
            Wanted::Predicate { .. } => Vec::new(),
        };
    };

    let Some(iri) = wanted_iri(db, entry, wanted) else {
        return Vec::new();
    };

    let text = doc_file.text(db);
    let range = locate_iri(text, &iri).unwrap_or(0..0);
    vec![NavigationTarget {
        file: doc_file,
        range,
    }]
}

/// The IRI to look for in the document.
fn wanted_iri(
    db: &dyn fossil_base::Db,
    entry: &TypeEntry,
    wanted: &Wanted<'_>,
) -> Option<String> {
    match wanted {
        Wanted::Shape => entry.shape_iri.as_ref().map(|s| s.to_string()),
        Wanted::Predicate { mapping, key } => {
            // `short_names` IS the rule — the last segment of
            // the predicate IRI, with `@rename` substituted first — so this
            // resolves a key exactly the way the checker resolves it, including
            // the renamed ones. Reimplementing `local_name` here would be the
            // seventh copy of it in this tree, and two of the six disagree.
            let shape = resolve_target_shape(db, *mapping).ok().flatten()?;
            let (table, _collisions) = shape.short_names(&entry.renames);
            table
                .iter()
                .find(|(short, _)| short.as_str() == key.as_str())
                .map(|(_, iri)| iri.to_string())
        }
    }
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

// ── The locator ──────────────────────────────────────────────────────────────

/// Where `iri` is written inside a shape document's text, as a byte range.
///
/// **This is a textual locator, not a parser.** The decode keeps no offsets (see
/// the module docs for why it may not cheaply grow any), so there is no exact
/// answer to hand back and this recovers one from the bytes. Two tiers:
///
/// 1. **the IRI verbatim.** ShExJ writes it out in full (`"predicate":
///    "http://example.org/name"`), ShExC writes it inside `<…>`, and the line
///    format `fossil_base::test_support` decodes writes it bare.
/// 2. **a prefixed name**, when the document declares a prefix whose expansion
///    the IRI starts with. `PREFIX shop: <https://shop.example/voc#>` makes
///    `https://shop.example/voc#Person` findable as `shop:Person`, which is how
///    every `.shex` in `apps/docs/programs/` actually spells its shapes.
///
/// Both tiers require the match to stand alone — `http://example.org/name` must
/// not match inside `http://example.org/nameOfThing`, and `shop:Person` must not
/// match inside `shop:PersonName`.
///
/// `None` when neither finds it, and the caller turns that into the top of the
/// file. What it cannot do is tell a shape's DECLARATION from a mention of it:
/// `@shop:Person` in a value position is found first if it comes first. That is
/// the price of not parsing, it is bounded (an editor lands a few lines off in
/// the right file), and the fix is a decoder that carries spans, not a smarter
/// regex.
fn locate_iri(text: &str, iri: &str) -> Option<Range<u32>> {
    if let Some(range) = find_standalone(text, iri) {
        return Some(range);
    }
    let (prefix, expansion) = declared_prefixes(text)
        .into_iter()
        .filter(|(_, expansion)| iri.starts_with(expansion.as_str()))
        // The longest expansion wins: two prefixes may nest
        // (`https://x/` and `https://x/voc#`) and only one of them
        // produces the name the document actually writes.
        .max_by_key(|(_, expansion)| expansion.len())?;
    let local = &iri[expansion.len()..];
    find_standalone(text, &format!("{prefix}:{local}"))
}

/// The first occurrence of `needle` in `text` that is not part of a longer name.
fn find_standalone(text: &str, needle: &str) -> Option<Range<u32>> {
    if needle.is_empty() {
        return None;
    }
    let mut from = 0usize;
    while let Some(hit) = text[from..].find(needle) {
        let start = from + hit;
        let end = start + needle.len();
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !is_name_char(c));
        if after_ok {
            return Some(u32::try_from(start).ok()?..u32::try_from(end).ok()?);
        }
        from = start + 1;
    }
    None
}

/// A character that can continue an IRI's last segment or a prefixed name's
/// local part. `:` and `/` are deliberately excluded — a hit followed by either
/// is a different IRI, not a longer name, and `<http://example.org/name>` must
/// match with the `>` after it.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
}

/// The `prefix → expansion` pairs a document declares.
///
/// Recognises the two spellings the corpus contains: SPARQL/ShExC's
/// `PREFIX p: <iri>` (case-insensitively — ShExC accepts both) and Turtle's
/// `@prefix p: <iri> .`, which is what a SHACL document written in Turtle uses.
/// ShExJ declares none and needs none: it writes every IRI out in full, so tier
/// 1 already answers for it.
///
/// This is the one piece of schema-language syntax this module knows, and it is
/// here rather than behind the decoder seam on purpose: it feeds a locator whose
/// failure mode is a slightly-off cursor. Nothing downstream reads it, no
/// diagnostic quotes it, and the checker resolves every one of these IRIs
/// through the real decoder.
fn declared_prefixes(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let rest = if let Some(rest) = trimmed.strip_prefix('@') {
            strip_keyword(rest, "prefix")
        } else {
            strip_keyword(trimmed, "prefix")
        };
        let Some(rest) = rest else { continue };
        let rest = rest.trim_start();
        let Some((name, rest)) = rest.split_once(':') else {
            continue;
        };
        if name.contains(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let Some(expansion) = rest.strip_prefix('<').and_then(|r| r.split('>').next()) else {
            continue;
        };
        out.push((name.to_string(), expansion.to_string()));
    }
    out
}

/// `s` with a leading case-insensitive `keyword` removed, when the keyword is
/// followed by whitespace.
fn strip_keyword<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    let head = s.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &s[keyword.len()..];
    rest.starts_with(char::is_whitespace).then_some(rest)
}

/// Push a target only if an equal one is not already present, so the LSP does
/// not show a doubled location.
fn push_unique(targets: &mut Vec<NavigationTarget>, target: NavigationTarget) {
    if !targets.contains(&target) {
        targets.push(target);
    }
}

// `name_candidates` lived here, and so did `prefix_decl_range` under it. The
// first climbed from a leaf token to the enclosing `IRI_EXPR` so a cursor on
// `ex` or on `Person` both resolved `ex:Person`; the second found the byte range
// of a `prefix <name>: <iri>` line. Neither has a node to walk any more.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    // -- the locator, which is the half that has no database in it -----------

    const SHEXC: &str = "\
PREFIX shop: <https://shop.example/voc#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>

shop:Person {
  shop:email xsd:string ;
  shop:name  xsd:string
}
";

    #[test]
    fn a_prefixed_shape_is_located_through_the_documents_own_prefix_table() {
        let range =
            locate_iri(SHEXC, "https://shop.example/voc#Person").expect("shop:Person is written");
        assert_eq!(&SHEXC[range.start as usize..range.end as usize], "shop:Person");
        // And it is the DECLARATION line, not one of the two `shop:` predicates.
        assert!(SHEXC[..range.start as usize].ends_with("\n\n"));
    }

    #[test]
    fn a_prefixed_predicate_is_located() {
        let range = locate_iri(SHEXC, "https://shop.example/voc#name").expect("shop:name");
        assert_eq!(&SHEXC[range.start as usize..range.end as usize], "shop:name");
    }

    /// The bug a naive `find` has: `shop:name` is a substring of nothing here,
    /// but `shop:email`'s local part starts the same way as a longer one would.
    /// The guard is the character AFTER the match.
    #[test]
    fn a_longer_name_is_not_a_match() {
        let doc = "PREFIX p: <http://e/>\np:nameOfThing xsd:string ;\np:name xsd:string\n";
        let range = locate_iri(doc, "http://e/name").expect("p:name is written");
        assert_eq!(&doc[range.start as usize..range.end as usize], "p:name");
        assert!(
            doc[..range.start as usize].contains("nameOfThing"),
            "the first (longer) occurrence must have been skipped"
        );
    }

    /// ShExJ writes every IRI out, so tier 1 answers without a prefix table.
    #[test]
    fn a_verbatim_iri_is_located_without_any_prefix_declaration() {
        let doc = "{ \"id\": \"http://example.org/Person\" }";
        let range = locate_iri(doc, "http://example.org/Person").expect("written in full");
        assert_eq!(
            &doc[range.start as usize..range.end as usize],
            "http://example.org/Person"
        );
    }

    /// The line format `fossil_base::test_support` decodes, which is what most
    /// of the workspace's tests write.
    #[test]
    fn the_line_format_is_located() {
        let doc = "shape http://example.org/Person\nprop http://example.org/name - 1 1\n";
        let shape = locate_iri(doc, "http://example.org/Person").expect("the shape line");
        assert_eq!(shape.start, 6, "just past `shape `");
        let pred = locate_iri(doc, "http://example.org/name").expect("the prop line");
        assert!(pred.start > shape.end, "the predicate comes after the shape");
    }

    #[test]
    fn an_iri_the_document_does_not_write_is_not_located() {
        assert!(locate_iri(SHEXC, "https://shop.example/voc#absent").is_none());
        assert!(locate_iri(SHEXC, "http://elsewhere.example/Person").is_none());
    }

    #[test]
    fn turtle_prefixes_are_read_too() {
        let doc = "@prefix ex: <http://example.org/> .\nex:Person a sh:NodeShape .\n";
        let range = locate_iri(doc, "http://example.org/Person").expect("ex:Person");
        assert_eq!(&doc[range.start as usize..range.end as usize], "ex:Person");
    }

    /// Two prefixes where one expansion extends the other. Only the longer one
    /// produces the name the document writes.
    #[test]
    fn the_longest_matching_prefix_wins() {
        let doc = "PREFIX a: <https://x/>\nPREFIX b: <https://x/voc#>\nb:Person {}\n";
        let range = locate_iri(doc, "https://x/voc#Person").expect("b:Person");
        assert_eq!(&doc[range.start as usize..range.end as usize], "b:Person");
    }
}
