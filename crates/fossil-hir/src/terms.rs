//! The two lexical checks the checker makes of an RDF term a program writes:
//! that a language tag is well-formed, and that an IRI is not a prefixed name.
//!
//! Both are about strings that the grammar accepts and RDF does not mean.
//! `"x"@abcdefghi` passes `LANGTAG`'s lexical rule and is not BCP 47; `"rdfs:label"`
//! is a well-formed absolute IRI whose scheme is `rdfs`, and nothing but this
//! check would say it was a mistake (`/docs/design/labels`).

/// Is `tag` a well-formed BCP 47 language tag (RFC 5646 §2.2.9)? RDF 1.1
/// Concepts §3.3 requires it of every language-tagged string. The writer asks
/// the same question of a tag read from the data, through this function.
#[must_use]
pub fn well_formed_language_tag(tag: &str) -> bool {
    oxilangtag::LanguageTag::parse(tag).is_ok()
}

/// The prefixes of the W3C's RDFa Core Initial Context
/// (<https://www.w3.org/2011/rdfa-context/rdfa-1.1>) and the namespaces they
/// abbreviate. It is the list a person writing `rdfs:label` has in mind, which
/// is why it is the list a prefixed name is recognised by. Nothing EXPANDS
/// through it: the context maps `schema:` to `http://schema.org/` while every
/// program here writes `https://schema.org/`, so an expansion would mint an IRI
/// nobody meant ([discarded](/docs/design/discarded)).
const RDFA_INITIAL_CONTEXT: &[(&str, &str)] = &[
    ("as", "https://www.w3.org/ns/activitystreams#"),
    ("cc", "http://creativecommons.org/ns#"),
    ("csvw", "http://www.w3.org/ns/csvw#"),
    ("ctag", "http://commontag.org/ns#"),
    ("dc", "http://purl.org/dc/terms/"),
    ("dc11", "http://purl.org/dc/elements/1.1/"),
    ("dcat", "http://www.w3.org/ns/dcat#"),
    ("dcterms", "http://purl.org/dc/terms/"),
    ("dqv", "http://www.w3.org/ns/dqv#"),
    ("duv", "https://www.w3.org/ns/duv#"),
    ("foaf", "http://xmlns.com/foaf/0.1/"),
    ("gr", "http://purl.org/goodrelations/v1#"),
    ("grddl", "http://www.w3.org/2003/g/data-view#"),
    ("ical", "http://www.w3.org/2002/12/cal/icaltzd#"),
    ("jsonld", "http://www.w3.org/ns/json-ld#"),
    ("ldp", "http://www.w3.org/ns/ldp#"),
    ("ma", "http://www.w3.org/ns/ma-ont#"),
    ("oa", "http://www.w3.org/ns/oa#"),
    ("odrl", "http://www.w3.org/ns/odrl/2/"),
    ("og", "http://ogp.me/ns#"),
    ("org", "http://www.w3.org/ns/org#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
    ("prov", "http://www.w3.org/ns/prov#"),
    ("qb", "http://purl.org/linked-data/cube#"),
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfa", "http://www.w3.org/ns/rdfa#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("rev", "http://purl.org/stuff/rev#"),
    ("rif", "http://www.w3.org/2007/rif#"),
    ("rr", "http://www.w3.org/ns/r2rml#"),
    ("schema", "http://schema.org/"),
    ("sd", "http://www.w3.org/ns/sparql-service-description#"),
    ("sioc", "http://rdfs.org/sioc/ns#"),
    ("skos", "http://www.w3.org/2004/02/skos/core#"),
    ("skosxl", "http://www.w3.org/2008/05/skos-xl#"),
    ("sosa", "http://www.w3.org/ns/sosa/"),
    ("ssn", "http://www.w3.org/ns/ssn/"),
    ("time", "http://www.w3.org/2006/time#"),
    ("v", "http://rdf.data-vocabulary.org/#"),
    ("vcard", "http://www.w3.org/2006/vcard/ns#"),
    ("void", "http://rdfs.org/ns/void#"),
    ("wdr", "http://www.w3.org/2007/05/powder#"),
    ("wdrs", "http://www.w3.org/2007/05/powder-s#"),
    ("xhv", "http://www.w3.org/1999/xhtml/vocab#"),
    ("xml", "http://www.w3.org/XML/1998/namespace"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
];

/// When `iri` is a prefixed name — its scheme is a prefix of the RDFa Core
/// Initial Context — the IRI it abbreviates there, which the refusal names.
/// `None` for anything else, including every `http:` and `https:` IRI.
#[must_use]
pub fn prefixed_name(iri: &str) -> Option<String> {
    let (scheme, local) = iri.split_once(':')?;
    if local.starts_with("//") {
        return None;
    }
    RDFA_INITIAL_CONTEXT
        .iter()
        .find(|(prefix, _)| *prefix == scheme)
        .map(|(_, namespace)| format!("{namespace}{local}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_held_to_bcp_47() {
        for good in ["en", "es", "pl", "en-US", "de-CH-1996", "zh-Hant-TW", "x-private"] {
            assert!(well_formed_language_tag(good), "{good}");
        }
        // `english-language` is well-formed — a five-to-eight letter
        // language and a variant — whether or not the registry knows it:
        // well-formedness is the syntax, §2.2.9's first half.
        assert!(well_formed_language_tag("english-language"));
        for bad in ["", "abcdefghi", "en_US", "-en", "en--us"] {
            assert!(!well_formed_language_tag(bad), "{bad}");
        }
    }

    #[test]
    fn a_prefixed_name_is_recognised_and_named_and_an_iri_is_not() {
        assert_eq!(
            prefixed_name("rdfs:label").as_deref(),
            Some("http://www.w3.org/2000/01/rdf-schema#label")
        );
        assert_eq!(
            prefixed_name("schema:name").as_deref(),
            Some("http://schema.org/name")
        );
        assert_eq!(prefixed_name("https://schema.org/name"), None);
        assert_eq!(prefixed_name("urn:isbn:0451450523"), None);
        assert_eq!(prefixed_name("label"), None);
    }
}
