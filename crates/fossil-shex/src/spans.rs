//! Where a `ShExC` document DECLARES a predicate, as a byte range.
//!
//! One diagnostic wants this and three programs in the conformance set are
//! written around it: `` `total` expects Float, and this is String `` says what
//! is wrong on the program's line, and the other half of the sentence —
//! `shop:Order declares shop:total as xsd:float` — is a line of `shape.shex`.
//! Without a range there is nothing to underline, so the message carried the
//! shape's answer as prose or not at all.
//!
//! # This is a LOOKUP over text, and it is deliberately not a parser
//!
//! `shex_ast` gives the meaning and this gives only the position. Its `Span` is
//! `nom_locate` and appears in `LocatedParseError` alone — the AST nodes carry
//! no offsets, so the decoded schema cannot say where anything was written, and
//! the alternatives were to patch rudof or to read the text.
//!
//! Reading the text is safe HERE and would not be safe as a parser, because of
//! the direction the two are used in. Nothing asks this module what a document
//! MEANS. It is handed a shape and a predicate that the AST already resolved,
//! and asked where they are; when it cannot find them it answers `None` and the
//! caller emits the diagnostic it emitted before, without the second label. A
//! disagreement between this and the AST costs a label. A disagreement between
//! a second parser and the AST would cost a wrong answer.
//!
//! **The spellings are rudof's own**, via `PrefixMap::qualify` — so there is no
//! second reader of `PREFIX` declarations, which is the one place where reading
//! the text twice could produce a wrong IRI rather than a missing range.
//!
//! # What it does not do
//!
//! `ShExJ` — the JSON interchange syntax — gets no spans. `ShExDescriptor` only
//! keeps the source for the compact path, and a JSON document's positions would
//! be positions in JSON, which is not what a reader of `ShExC` is looking at.
//!
//! SHACL gets none either, and that one is a DECISION rather than an omission:
//! Turtle gives no textual scope to search inside, so the equivalent lookup
//! would have to parse. `fossil_descriptors_output::shacl`'s «Positions»
//! section carries the measurement and the upstream change that reverses it.

use fossil_graph_schema::Span;

/// The range of `predicate` where `shape` declares it, both spelled as the
/// document spells them (`shop:Order`, `shop:total`).
///
/// The predicate must be at the START of a triple constraint — preceded, past
/// whitespace, by the shape's `{`, a `;` or a `|`. That is what keeps
/// `shop:buyer @shop:Person` from answering for the predicate `shop:Person`:
/// a value reference sits after `@`, never at the head of a constraint. The
/// corpus has that exact pair in `errors/unknown-field/shop.shex`.
#[must_use]
pub fn predicate_span(src: &str, shape: &str, predicate: &str) -> Option<Span> {
    let body = shape_body(src, shape)?;
    let block = src.get(body.clone())?;
    let at = block
        .match_indices(predicate)
        .filter(|(i, _)| is_token_at(block, *i, predicate))
        .find(|(i, _)| heads_a_constraint(block, *i))
        .map(|(i, _)| i)?;
    let start = u32::try_from(body.start + at).ok()?;
    let end = start.checked_add(u32::try_from(predicate.len()).ok()?)?;
    Some(Span::new(start, end))
}

/// The byte range BETWEEN the braces of `shape`'s declaration.
///
/// The label is matched as a whole token so `shop:Order` does not answer for
/// `shop:OrderLine`, and the `{` must be the next non-whitespace byte after it
/// — a shape label is followed by its body and by nothing else.
fn shape_body(src: &str, shape: &str) -> Option<std::ops::Range<usize>> {
    for (i, _) in src.match_indices(shape) {
        if !is_token_at(src, i, shape) {
            continue;
        }
        let after = i + shape.len();
        let rest = src.get(after..)?;
        let open = rest.find(|c: char| !c.is_whitespace())?;
        if rest.as_bytes().get(open) != Some(&b'{') {
            continue;
        }
        let body_start = after + open + 1;
        let mut depth = 1usize;
        for (j, b) in src.get(body_start..)?.bytes().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(body_start..body_start + j);
                    }
                }
                _ => {}
            }
        }
        // An unclosed brace. The document does not parse either, and the
        // decoder has said so; there is nothing to underline.
        return None;
    }
    None
}

/// Is the match at `i` a whole token rather than the tail or head of a longer
/// one? A `ShExC` name may carry `:`, `-`, `_` and `.` besides alphanumerics,
/// and `@` before it is what makes a value reference rather than a predicate.
fn is_token_at(src: &str, i: usize, needle: &str) -> bool {
    let name_char = |c: char| c.is_alphanumeric() || matches!(c, ':' | '-' | '_' | '.');
    let before_ok = src[..i].chars().next_back().is_none_or(|c| !name_char(c));
    let after_ok = src[i + needle.len()..]
        .chars()
        .next()
        .is_none_or(|c| !name_char(c));
    before_ok && after_ok
}

/// Is the match at `i` at the head of a triple constraint — nothing but
/// whitespace between it and the start of the block, a `;` or a `|`?
fn heads_a_constraint(block: &str, i: usize) -> bool {
    block[..i]
        .chars()
        .rev()
        .find(|c| !c.is_whitespace())
        .is_none_or(|c| c == ';' || c == '|')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOP: &str = "\
PREFIX shop: <https://shop.example/voc#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>

shop:Person {
  shop:email xsd:string ;
  shop:name  xsd:string ;
  shop:phone xsd:string ?
}

shop:Order {
  shop:total xsd:float ;
  shop:buyer @shop:Person
}
";

    fn at<'a>(src: &'a str, shape: &str, predicate: &str) -> Option<&'a str> {
        predicate_span(src, shape, predicate)?.slice(src)
    }

    #[test]
    fn a_predicate_is_found_in_the_shape_that_declares_it() {
        assert_eq!(at(SHOP, "shop:Order", "shop:total"), Some("shop:total"));
        assert_eq!(at(SHOP, "shop:Person", "shop:email"), Some("shop:email"));
    }

    /// The whole point of scoping to the shape's braces: two shapes in one
    /// document, and a predicate belongs to one of them.
    #[test]
    fn a_predicate_of_another_shape_is_not_found() {
        assert_eq!(at(SHOP, "shop:Order", "shop:email"), None);
        assert_eq!(at(SHOP, "shop:Person", "shop:total"), None);
    }

    /// `shop:buyer @shop:Person` — the value reference is not a declaration,
    /// and the shape label `shop:Person` above it is outside these braces.
    #[test]
    fn a_value_reference_does_not_answer_for_a_predicate() {
        assert_eq!(at(SHOP, "shop:Order", "shop:Person"), None);
    }

    /// Two predicates whose LOCAL names collide are two IRIs and therefore two
    /// spellings, so each finds its own line. This is `errors/colliding-name`,
    /// and it is why the lookup takes the qualified spelling and not the local
    /// name.
    #[test]
    fn two_predicates_with_one_local_name_are_two_ranges() {
        const COLLIDING: &str = "\
PREFIX shop: <https://shop.example/voc#>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>

shop:Person {
  shop:name xsd:string ;
  foaf:name xsd:string
}
";
        let shop = predicate_span(COLLIDING, "shop:Person", "shop:name").expect("declared");
        let foaf = predicate_span(COLLIDING, "shop:Person", "foaf:name").expect("declared");
        assert_eq!(shop.slice(COLLIDING), Some("shop:name"));
        assert_eq!(foaf.slice(COLLIDING), Some("foaf:name"));
        assert!(shop.start < foaf.start, "in the order the document lists");
    }

    /// A prefix of a longer name is not the name.
    #[test]
    fn a_longer_name_is_not_a_match() {
        const NESTED: &str = "shop:OrderLine {\n  shop:totalling xsd:float\n}\n";
        assert_eq!(predicate_span(NESTED, "shop:Order", "shop:total"), None);
        assert_eq!(
            at(NESTED, "shop:OrderLine", "shop:totalling"),
            Some("shop:totalling")
        );
    }

    /// Nothing here panics on a document that does not parse — the decoder has
    /// already said why, and this answers `None` so the caller drops one label.
    #[test]
    fn a_document_that_does_not_parse_yields_no_range() {
        assert_eq!(
            predicate_span("shop:Order {", "shop:Order", "shop:total"),
            None
        );
        assert_eq!(predicate_span("", "shop:Order", "shop:total"), None);
        assert_eq!(
            predicate_span("shop:Order", "shop:Order", "shop:total"),
            None
        );
    }
}
