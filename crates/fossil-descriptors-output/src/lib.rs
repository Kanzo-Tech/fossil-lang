//! `fossil-descriptors-output` — the type-reading rows of the provider registry,
//! and the assembled table a compiling host installs.
//!
//! # What this crate is now
//!
//! [`SHEX`] and [`SHACL`] are [`fossil_base::Provider`] rows: a `name` (what a program writes
//! after `io.`), the extensions the row accepts, and a pure
//! `fn(&str, &str) -> Result<OutputShapes, Rejection>` under the *read types*
//! capability. [`PROVIDERS`] is those two spliced onto `fossil_base`'s data rows
//! — the whole registry, which a host hands out from `System::providers`.
//! `fossil_base::shape_document` picks a row **by the name the program wrote**
//! and calls it. The compiler reads the neutral vocabulary and never learns
//! which language produced it — the cut `0e6898d` made for `fossil-mir`, now
//! made for the checker.
//!
//! Ruling 13 of `SURFACE-PLAN.md` is why the two rows are here rather than one
//! here and one in `fossil-df`: a decoder lives beside its machinery, and both
//! machineries are shape-document parsers. `fossil-df` is where the SHACL walk
//! used to live, producing a `GraphSchema` — the OUTPUT model, one step past the
//! vocabulary the checker reads — which is precisely why `catalogue.fossil` could
//! type-check against nothing.
//!
//! # What it stopped being
//!
//! It used to re-export `fossil_shex`'s types from its own root
//! (`ShExDescriptor`, `ShapeBinding`, `ResolvedConstraint`, `ConstraintValue`,
//! `ShExLoweringError`, `OneOfRejection`, `SuggestionSeed`,
//! `generate_split_suggestion`, and a `Cardinality` enum that no longer exists),
//! so a consumer could reach `shex_ast` through a crate whose name says
//! "descriptors". It does not. A consumer that genuinely wants the `ShEx` AST
//! names [`fossil_shex`] directly and says so in its `Cargo.toml`; a consumer
//! that wants a decoded document takes [`SHEX`] and gets no `ShEx` type at all.
//!
//! # What is still here, and why
//!
//! [`OutputDescriptor`] and [`OutputDescriptorKind`] stay for now: `fossil-df`,
//! `fossil-df-wasm` and `fossil-engine` carry an `OutputDescriptorKind` into
//! the executor. They are the next thing to go — the kind enum's only live
//! method is `to_graph_schema`, which is the seam `apply_output_shape` already
//! takes directly.

mod generated;
pub mod kind;
pub mod shacl;

pub use generated::{PROVIDERS, SHACL, SHEX};
pub use kind::OutputDescriptorKind;
pub use shacl::{decode_shacl, subject_value};

use fossil_graph_schema::{OutputShapes, Rejection};
use fossil_shex::{ShExDescriptor, ShExLoweringError};

/// Decode a `ShEx` document — the `ShEx` row's `reads_types`.
///
/// Accepts **both** surface syntaxes: `ShExC` (compact, human-authored) and
/// `ShExJ` (JSON interchange), auto-detected by the first non-whitespace byte.
/// Both lower through the same constraint table, so the two forms of one schema
/// say the same thing.
///
/// They no longer produce EQUAL [`OutputShapes`], and the difference is
/// positions: [`fossil_graph_schema::PropertyConstraint::span`] says where the
/// document declares a predicate, and only the compact form has an answer —
/// offsets in JSON are not the document anyone is reading a report about.
/// `shexc_and_shexj_of_one_schema_decode_equal` compares modulo that, and
/// asserts the difference is real rather than both sides being empty.
///
/// The `uri` is unread. `ShEx`'s relative-reference base is a fixed sentinel
/// inside [`ShExDescriptor::from_shex_source`], which is what the compiler
/// already parsed against; threading the document's own URI in would change
/// which IRIs a `ShExC` document resolves to, and that is a decision, not a
/// tidy-up.
///
/// # Errors
///
/// [`Rejection::Malformed`] when neither parser accepts the text. Everything
/// else the decoder could not lower — a `OneOf`, a cyclic ref, an unresolved
/// ref — is **not** an error: it rides along in
/// [`OutputShapes::rejections`] beside the shapes that did lower.
pub fn decode_shex(_uri: &str, text: &str) -> Result<OutputShapes, Rejection> {
    match ShExDescriptor::from_shex_source(text) {
        Ok(descriptor) => Ok(descriptor.to_output_shapes()),
        Err(ShExLoweringError::MalformedSchema(message)) => Err(Rejection::Malformed(message)),
        // Unreachable: `from_shex_source` fails only at the parse, and a parse
        // failure is `MalformedSchema`. The other variants are collected during
        // the walk and reach `rejections()` on the `Ok` path.
        Err(other) => Err(Rejection::Malformed(format!("{other:?}"))),
    }
}

// `SHEX`, `SHACL` and `PROVIDERS` stood here as hand-written statics. They are
// generated from `catalogue.bnf` now — `mod generated`, above — and each one's
// argument moved into that file's `(* … *)` commentary.
//
// The generated file writes `decode_shex` and `decode_shacl` as Rust PATHS, so
// the compiler resolves them. That is the one thing `catalogue_parity.rs`, which
// this replaces, said it could not prove.

/// Output-side shape descriptor.
///
/// Implementations parse a raw descriptor blob (`ShEx`, future `SHACL`) and
/// expose shape membership / cardinality queries used by the type-checker
/// for backward shape inference.
pub trait OutputDescriptor: Send + Sync + std::fmt::Debug {
    /// Stable, lowercase, namespace-free identifier (e.g. `"shex"`, `"accept-all"`).
    ///
    /// Trait signature returns `&str` (not `&'static str`) so Phase 3+
    /// implementations can return dynamically-computed names.
    fn name(&self) -> &str;

    /// Phase 1: tells the type-checker whether the descriptor is a permissive
    /// "anything goes" pass-through (used for demos with no shape target).
    /// Phase 3 CORE-06 supersedes with shape-aware queries
    /// (`type_for_property`, `cardinality`, `is_closed`).
    fn accepts_anything(&self) -> bool;
}

/// Phase 1 stub: accept-all output descriptor.
///
/// Returns `true` for [`OutputDescriptor::accepts_anything`] — the type-checker
/// short-circuits backward shape inference. It survives as the `AcceptAll`
/// variant of [`OutputDescriptorKind`]: the fallback when no `ShEx` schema is
/// loaded.
///
/// Declared as a unit struct (`pub struct AcceptAllDescriptor;`) so the
/// [`OutputDescriptorKind::ACCEPT_ALL_DEFAULT`] inherent const is
/// const-evaluable.
#[derive(Debug, Default)]
pub struct AcceptAllDescriptor;

impl OutputDescriptor for AcceptAllDescriptor {
    // Phase 1 returns a literal; the trait signature stays `&str` for
    // Phase 3+ dynamic naming (see OutputDescriptor::name() doc).
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "accept-all"
    }

    fn accepts_anything(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use fossil_base::{Capability, provider};
    use fossil_graph_schema::PropertyConstraint;

    use super::*;

    #[test]
    fn accept_all_descriptor_says_yes() {
        let d = AcceptAllDescriptor;
        assert_eq!(d.name(), "accept-all");
        assert!(d.accepts_anything());
    }

    /// One schema, both surface syntaxes. `ShExC` is what a person writes and
    /// `ShExJ` is what a tool emits; if they did not decode to the same value,
    /// the choice of syntax would be a semantic choice.
    ///
    /// # The claim narrowed, and the narrowing is the point
    ///
    /// It compared the two whole values, and cannot any more:
    /// [`fossil_graph_schema::PropertyConstraint::span`] says WHERE the document
    /// declares a predicate, and a compact document has an answer where a JSON
    /// one has `None`. The two still say the same thing; they are no longer
    /// written in the same place, which is what a position is.
    ///
    /// So the comparison is modulo the position, spelled out rather than
    /// derived — clearing the field before comparing would make this test pass
    /// for a value the compiler never sees. The `assert!` below is the other
    /// half: the compact document does carry one, so the equality is not being
    /// bought by both sides being empty.
    #[test]
    fn shexc_and_shexj_of_one_schema_decode_equal() {
        /// The two documents modulo POSITION — the one thing they legitimately
        /// disagree about. Spelled out rather than derived: clearing the field
        /// before comparing would make this pass for a value the compiler never
        /// sees.
        fn said(shapes: &OutputShapes) -> Vec<(String, Vec<PropertyConstraint>)> {
            shapes
                .shapes()
                .map(|s| {
                    let properties = s
                        .properties
                        .iter()
                        .map(|p| PropertyConstraint {
                            span: None,
                            ..p.clone()
                        })
                        .collect();
                    (s.iri.clone(), properties)
                })
                .collect()
        }

        const SHEXC: &str = "\
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string ;
  ex:age xsd:integer ? ;
  ex:knows @ex:Person *
}
";
        const SHEXJ: &str = r#"{
          "@context": "http://www.w3.org/ns/shex.jsonld",
          "type": "Schema",
          "shapes": [
            {
              "type": "ShapeDecl",
              "id": "http://example.org/Person",
              "shapeExpr": {
                "type": "Shape",
                "expression": {
                  "type": "EachOf",
                  "expressions": [
                    {
                      "type": "TripleConstraint",
                      "predicate": "http://example.org/name",
                      "valueExpr": {
                        "type": "NodeConstraint",
                        "datatype": "http://www.w3.org/2001/XMLSchema#string"
                      }
                    },
                    {
                      "type": "TripleConstraint",
                      "predicate": "http://example.org/age",
                      "valueExpr": {
                        "type": "NodeConstraint",
                        "datatype": "http://www.w3.org/2001/XMLSchema#integer"
                      },
                      "min": 0,
                      "max": 1
                    },
                    {
                      "type": "TripleConstraint",
                      "predicate": "http://example.org/knows",
                      "valueExpr": "http://example.org/Person",
                      "min": 0,
                      "max": -1
                    }
                  ]
                }
              }
            }
          ]
        }"#;

        let compact = decode_shex("person.shexc", SHEXC).expect("ShExC decodes");
        let json = decode_shex("person.shexj", SHEXJ).expect("ShExJ decodes");

        assert_eq!(
            said(&compact),
            said(&json),
            "the syntax is not part of what a schema says"
        );
        assert_eq!(
            compact.rejections(),
            json.rejections(),
            "and neither is what it could not lower"
        );

        // Not vacuous: the value actually carries the schema.
        let person = compact
            .lookup("http://example.org/Person")
            .expect("ex:Person");
        assert_eq!(person.properties.len(), 3);
        // And the position IS the difference — not both sides being empty.
        assert!(
            person.properties[0].span.is_some(),
            "a compact document says where it declares a predicate"
        );
        assert!(
            json.lookup("http://example.org/Person")
                .expect("ex:Person")
                .properties[0]
                .span
                .is_none(),
            "and a JSON one does not: its offsets would be offsets in JSON"
        );
        assert_eq!(
            person.properties[2].targets,
            ["http://example.org/Person"],
            "`@ex:Person` is an edge back to the same shape"
        );
    }

    /// A parse failure is a [`Rejection::Malformed`] the caller can put in front
    /// of a user, not a panic and not a silent empty document.
    #[test]
    fn text_that_parses_as_neither_syntax_is_malformed() {
        let err = decode_shex("broken.shex", "{ this is not a schema").expect_err("must fail");
        assert!(matches!(err, Rejection::Malformed(_)), "got {err:?}");
    }

    /// A `OneOf` is **not** a decode failure: the shapes that lowered come back,
    /// and what did not rides along in `rejections()`. Collapsing that to an
    /// `Err` would throw away the rest of the document.
    #[test]
    fn a_rejected_construct_rides_along_rather_than_failing_the_decode() {
        const SHEXC: &str = "\
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Contact {
  ex:email xsd:string | ex:phone xsd:string
}
";
        let doc = decode_shex("contact.shexc", SHEXC).expect("the document parses");
        assert_eq!(
            doc.rejections(),
            [Rejection::Disjunction {
                shape_iri: "http://example.org/Contact".into(),
                disjuncts: vec![
                    vec!["http://example.org/email".to_string()],
                    vec!["http://example.org/phone".to_string()],
                ],
            }]
        );
        assert!(
            doc.lookup("http://example.org/Contact").is_some(),
            "the shape is still declared, it just has nothing the checker can use"
        );
    }

    /// The row is what a host installs, and it is reached **by name**. The
    /// extension is what the row then checks about the document it was handed.
    #[test]
    fn the_shex_row_is_reached_by_name_and_checks_its_own_extension() {
        assert_eq!(provider(PROVIDERS, "io.shex"), Some(&SHEX));
        assert_eq!(SHEX.name, "shex");
        for uri in [
            "shapes/shop.shex",
            "shop.shexj",
            "shop.shexc",
            "SHOP.ShEx", // matched lowercased
        ] {
            assert!(SHEX.accepts(uri), "the shex row declined `{uri}`");
        }
        assert!(!SHEX.accepts("shapes.ttl"));
        // `catalogue.ttl` is the case ruling 13 leads with, and the row's answer
        // is a bool. The SENTENCE it turns into is
        // `fossil_hir::refusals::decline_extension`, tested there and asserted
        // end-to-end over this very table in
        // `fossil-engine/tests/provider_registry.rs`.
        assert!(!SHEX.accepts("catalogue.ttl"));
    }

    /// **The pair that behaved identically until now.** `io.shex("x.ttl")` and
    /// `io.shacl("x.ttl")` were one answer, because the extension chose the row
    /// and only one row claimed `.ttl`.
    #[test]
    fn shex_and_shacl_are_two_rows_and_the_name_is_what_separates_them() {
        let shex = provider(PROVIDERS, "io.shex").expect("installed");
        let shacl = provider(PROVIDERS, "io.shacl").expect("installed");
        assert_ne!(shex, shacl);
        assert!(!shex.accepts("catalogue.ttl"));
        assert!(shacl.accepts("catalogue.ttl"));
        assert!(shacl.provides(Capability::ReadTypes));
        assert!(!shacl.provides(Capability::ReadRows));
    }

    /// A compiling host's table has every `io.*` the language has, and the two
    /// halves are separable by capability alone.
    #[test]
    fn the_installed_table_carries_the_data_rows_too() {
        for name in ["csv", "json", "parquet", "rdf", "shex", "shacl"] {
            assert!(provider(PROVIDERS, name).is_some(), "`io.{name}` missing");
        }
        let csv = provider(PROVIDERS, "io.csv").expect("installed");
        assert!(!csv.provides(Capability::ReadTypes));
        let readers: Vec<&str> = PROVIDERS
            .iter()
            .filter(|p| p.provides(Capability::ReadTypes))
            .map(|p| p.name)
            .collect();
        assert_eq!(readers, ["shex", "shacl"], "the two type readers, in order");
    }

    /// `.ttl` is claimed by two rows with two different capabilities, and that
    /// is only expressible because dispatch is by name.
    #[test]
    fn one_extension_two_capabilities() {
        let rdf = provider(PROVIDERS, "io.rdf").expect("installed");
        let shacl = provider(PROVIDERS, "io.shacl").expect("installed");
        assert!(rdf.accepts("g.ttl") && shacl.accepts("g.ttl"));
        assert!(rdf.provides(Capability::ReadRows));
        assert!(shacl.provides(Capability::ReadTypes));
    }
}
