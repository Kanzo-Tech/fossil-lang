//! Reading the policy document.
//!
//! # A JSON-LD document read with a JSON parser
//!
//! The policy is JSON-LD and this is not a JSON-LD processor. It does not expand
//! terms against a remote `@context`, it does not follow `@graph`, it does not
//! resolve blank nodes, and it will not accept a document that says the same
//! thing a different way.
//!
//! That is the same call `apps/corpus/guards/manifest.mjs` makes about the
//! manifest, for the same reason and with the same price. Full JSON-LD
//! processing means a dependency with a network fetcher in it — remote contexts
//! are how JSON-LD works — and a compile-time document whose meaning depends on
//! `w3.org` being up is not a document, it is an outage. `/docs/design/discarded`
//! already carries that argument against resolving a namespace prefix over the
//! network at compile time; a context is the same class of thing.
//!
//! So: **a fixed shape, and everything outside it is refused with the reason.**
//! A reader that silently understands half a policy is worse than one that
//! stops, because the half it dropped is the half that said "do not publish
//! this".
//!
//! # What it refuses rather than guesses at
//!
//! - A `@context` that is not the ODRL context plus a local mapping. The ODRL
//!   context aliases `uid` to `@id` and types `leftOperand`, `operator` and
//!   `action` as `@vocab`, so a bare string means something specific; a
//!   different context makes every one of those a guess.
//! - A `leftOperand` outside [`crate::profile`]'s six.
//! - A `rightOperand` that is not a classification this profile defines.
//! - An `operator` other than `odrl:eq`, `odrl:gteq` and `odrl:lteq`.
//! - Any nesting the shapes below do not have.
//!
//! # The shape
//!
//! ```json
//! {
//!   "@context": ["http://www.w3.org/ns/odrl.jsonld",
//!                {"fossil": "https://fossil-lang.org/ns/privacy#",
//!                 "dpv": "https://w3id.org/dpv#"}],
//!   "@type": "Set",
//!   "uid": "https://example.org/policies/persons-v1",
//!   "profile": "https://fossil-lang.org/ns/privacy/v1",
//!   "permission": [{
//!     "target": "Person",
//!     "action": "use",
//!     "constraint": [
//!       {"leftOperand": "fossil:anonymityK", "operator": "gteq", "rightOperand": 5},
//!       {"leftOperand": "fossil:absentQuasiIdentifier", "operator": "eq",
//!        "rightOperand": "suppress"},
//!       {"leftOperand": "fossil:suppressionBudget", "operator": "lteq",
//!        "rightOperand": 20000},
//!       {"and": [
//!         {"leftOperand": "fossil:attribute", "operator": "eq",
//!          "rightOperand": "https://example.org/birthYear"},
//!         {"leftOperand": "fossil:classification", "operator": "eq",
//!          "rightOperand": "fossil:QuasiIdentifier"},
//!         {"leftOperand": "fossil:generalization", "operator": "eq",
//!          "rightOperand": {"kind": "numeric", "buckets": [0, 18, 30, 45, 65, 80],
//!                           "presentation": "enclosing_bucket"}}
//!       ]}
//!     ]
//!   }],
//!   "prohibition": [{
//!     "target": "Person",
//!     "action": "use",
//!     "constraint": [{"leftOperand": "fossil:attribute", "operator": "eq",
//!                     "rightOperand": "https://example.org/nationalId"}]
//!   }]
//! }
//! ```
//!
//! The `odrl:and` is not decoration. A `Constraint` is one
//! `(leftOperand, operator, rightOperand)` triple and "this attribute has that
//! classification" is two facts, so it is a `LogicalConstraint` over two
//! constraints — which is ODRL's own way of saying it and needs no term of ours.
//! Three, above, because "and is generalised by that hierarchy" is a third fact
//! about the same attribute; the reader requires the `fossil:attribute` and at
//! least one of the other two, so a document may split them across two `and`s
//! or write a classification without a hierarchy, which is what every policy
//! written before `fossil:generalization` existed does.
//!
//! That last `rightOperand` is an **object** rather than a string, which is the
//! one place this reader's shape steps outside what a `@vocab`-typed term
//! expects. It is deliberate and it is why the hierarchy is not a term: the
//! value is data the policy author ships, it is the exact JSON
//! `fossil-kanon`'s `hierarchies/*.json` files hold, and neither a registry
//! name nor a fetchable IRI would let them paste one in. See
//! [`crate::profile::GENERALIZATION`].

use std::collections::BTreeMap;

use serde_json::Value;

use crate::profile::{self, PROFILE};
use crate::{AbsentQuasiIdentifier, Classification, PrivacyPolicy, ShapeRule};

/// The ODRL 2.2 JSON-LD context. The only one accepted, because every bare
/// string in the document means what this context says it means.
pub const ODRL_CONTEXT: &str = "http://www.w3.org/ns/odrl.jsonld";

/// Why a policy document was refused.
///
/// Every variant names the term or the shape it stopped on. ODRL §3.2 is the
/// authority for stopping rather than continuing: *"if the ODRL Processing
/// system does not recognise the ODRL Profile identifier(s) then it MUST stop
/// processing the policy"*, and the same reasoning covers a term inside a
/// profile it does recognise.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    /// The document is not JSON.
    #[error("the policy document is not JSON: {0}")]
    NotJson(String),
    /// The document does not declare the profile, or declares another.
    #[error(
        "the policy declares `odrl:profile` {found:?} and this reader implements {PROFILE}. \
         ODRL requires a processor that does not recognise a profile to stop rather than guess"
    )]
    WrongProfile {
        /// What the document said, if anything.
        found: Option<String>,
    },
    /// `@context` is not the ODRL context plus a local mapping.
    #[error(
        "the policy's `@context` does not include {ODRL_CONTEXT}. Every bare term in an ODRL \
         document means what that context says it means — `uid` is `@id`, `leftOperand` is \
         `@vocab` — so a document without it is one this reader would be guessing at"
    )]
    ForeignContext,
    /// A required key is missing.
    #[error("the policy has no `{0}`")]
    Missing(&'static str),
    /// A term this profile does not define.
    #[error("`{term}` is not one of this profile's terms{}", context_of(.at))]
    UnknownTerm {
        /// The offending IRI or compact term.
        term: String,
        /// Where it appeared.
        at: Option<String>,
    },
    /// A value of the wrong shape.
    #[error("`{key}` should be {want}, and is {got}")]
    BadValue {
        /// The key.
        key: String,
        /// What was expected.
        want: &'static str,
        /// What was there.
        got: String,
    },
    /// The policy contradicts itself.
    #[error(
        "`{shape}` puts `{attribute}` in its quasi-identifier set and classifies it as something \
         else. The set is the combination the bound is computed over, and a member of it that is \
         not a quasi-identifier is a policy disagreeing with itself"
    )]
    SetContradictsClassification {
        /// The shape.
        shape: String,
        /// The offending attribute.
        attribute: String,
    },

    /// A hierarchy was declared for something the bound is not computed over.
    #[error(
        "`{shape}` declares a generalisation for `{attribute}` and does not classify it as a \
         quasi-identifier. Generalising a column the bound is not computed over costs the release \
         its resolution and buys it no anonymity — the k is over the quasi-identifier set, and \
         `{attribute}` is not in it"
    )]
    GeneralizationOnNonQuasiIdentifier {
        /// The shape.
        shape: String,
        /// The offending attribute.
        attribute: String,
    },

    /// A hierarchy whose `kind` this vocabulary does not admit.
    #[error(
        "`{shape}` generalises `{attribute}` by a `{kind:?}` hierarchy, and this profile admits \
         {}. The hierarchy is the object `fossil-kanon` deserialises, written inline",
        crate::profile::HIERARCHY_KINDS.join(", ")
    )]
    UnknownHierarchyKind {
        /// The shape.
        shape: String,
        /// The attribute it was declared for.
        attribute: String,
        /// The `kind` that was written, if there was one at all.
        kind: Option<String>,
    },
}

fn context_of(at: &Option<String>) -> String {
    at.as_ref()
        .map_or_else(String::new, |a| format!(" (in `{a}`)"))
}

/// Parse a policy document.
///
/// # Errors
/// [`PolicyError`] for anything outside the shape in this module's header. The
/// reader never repairs and never defaults.
pub fn parse(text: &str) -> Result<PrivacyPolicy, PolicyError> {
    let doc: Value = serde_json::from_str(text).map_err(|e| PolicyError::NotJson(e.to_string()))?;

    let prefixes = context_prefixes(&doc)?;
    let expand = |s: &str| expand_term(s, &prefixes);

    let profile_iri = doc.get("profile").and_then(Value::as_str).map(&expand);
    if profile_iri.as_deref() != Some(PROFILE) {
        return Err(PolicyError::WrongProfile { found: profile_iri });
    }
    let uid = doc
        .get("uid")
        .and_then(Value::as_str)
        .ok_or(PolicyError::Missing("uid"))?
        .to_string();

    let mut k: Option<u64> = None;
    let mut absent: Option<AbsentQuasiIdentifier> = None;
    let mut budget: Option<u64> = None;
    // Ordered, so that two runs over one document produce one policy. A
    // `HashMap` here would make `shapes` order depend on the hash seed, and the
    // manifest's `quasi_identifiers` string is derived from it.
    let mut shapes: BTreeMap<String, ShapeRule> = BTreeMap::new();

    for (key, prohibiting) in [("permission", false), ("prohibition", true)] {
        for rule in doc.get(key).and_then(Value::as_array).into_iter().flatten() {
            let target = rule
                .get("target")
                .and_then(Value::as_str)
                .ok_or(PolicyError::Missing("target"))?;
            let entry = shapes
                .entry(target.to_string())
                .or_insert_with(|| ShapeRule {
                    shape: target.to_string(),
                    classification: Vec::new(),
                    quasi_identifiers: Vec::new(),
                    prohibited: Vec::new(),
                    generalizations: Vec::new(),
                });
            for constraint in rule
                .get("constraint")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                read_constraint(
                    constraint,
                    &expand,
                    prohibiting,
                    entry,
                    &mut k,
                    &mut absent,
                    &mut budget,
                )?;
            }
        }
    }

    let mut shapes: Vec<ShapeRule> = shapes.into_values().collect();
    for shape in &mut shapes {
        // A set that is not a subset of the quasi-identifier classification is
        // a document saying two things. Caught here rather than at verification
        // time because it is a defect in the policy, not in the corpus, and the
        // person who can fix it is reading this message and not a run log.
        for attribute in &shape.quasi_identifiers {
            if shape.classify(Some(attribute), attribute) != Classification::QuasiIdentifier {
                return Err(PolicyError::SetContradictsClassification {
                    shape: shape.shape.clone(),
                    attribute: attribute.clone(),
                });
            }
        }
        // A hierarchy for a column the bound is not computed over. Caught here
        // and not at write time for the same reason as the rule above: it is a
        // defect in the policy, the corpus has nothing to do with it, and the
        // person who can fix it is reading this message. It is checked AFTER
        // the loop above so that a document which is wrong in both ways is
        // reported on the contradiction first — that one explains this one.
        for (attribute, _) in &shape.generalizations {
            if shape.classify(Some(attribute), attribute) != Classification::QuasiIdentifier {
                return Err(PolicyError::GeneralizationOnNonQuasiIdentifier {
                    shape: shape.shape.clone(),
                    attribute: attribute.clone(),
                });
            }
        }
        shape.classification.sort_by(|a, b| a.0.cmp(&b.0));
        shape.quasi_identifiers.sort();
        shape.prohibited.sort();
        shape.generalizations.sort_by(|a, b| a.0.cmp(&b.0));
    }

    Ok(PrivacyPolicy {
        uid,
        profile: PROFILE.to_string(),
        k: k.ok_or(PolicyError::Missing("a fossil:anonymityK constraint"))?,
        // The one parameter with no default, on purpose. Both readings are
        // defensible, so a document that does not choose has not said what its
        // own `k` means.
        absent_quasi_identifier: absent.ok_or(PolicyError::Missing(
            "a fossil:absentQuasiIdentifier constraint",
        ))?,
        // This one defaults, and to zero: a policy that says nothing about
        // suppression is a policy that allows none, which is the reading that
        // cannot silently weaken a bound.
        suppression_budget_ppm: budget.unwrap_or(0),
        shapes,
    })
}

/// One `odrl:Constraint`, or one `odrl:and` over two of them.
fn read_constraint(
    constraint: &Value,
    expand: &impl Fn(&str) -> String,
    prohibiting: bool,
    shape: &mut ShapeRule,
    k: &mut Option<u64>,
    absent: &mut Option<AbsentQuasiIdentifier>,
    budget: &mut Option<u64>,
) -> Result<(), PolicyError> {
    // A `LogicalConstraint`: the two-fact case, «this attribute has that
    // classification». ODRL's own shape for it, so it costs no term of ours.
    if let Some(pair) = constraint.get("and").and_then(Value::as_array) {
        let mut attribute = None;
        let mut classification = None;
        let mut generalization = None;
        for part in pair {
            let (left, right) = operand(part, expand)?;
            match left.as_str() {
                profile::ATTRIBUTE => attribute = Some(expand(as_str(&right, "rightOperand")?)),
                profile::CLASSIFICATION => {
                    let iri = expand(as_str(&right, "rightOperand")?);
                    classification = Some(profile::classification::of(&iri).ok_or(
                        PolicyError::UnknownTerm {
                            term: iri,
                            at: Some("fossil:classification".to_string()),
                        },
                    )?);
                }
                // The hierarchy, verbatim: not expanded, not interpreted, not
                // converted. It is an object rather than a term, and the crate
                // that can act on one is not this crate — see
                // `profile::GENERALIZATION`.
                profile::GENERALIZATION => generalization = Some(right),
                other => {
                    return Err(PolicyError::UnknownTerm {
                        term: other.to_string(),
                        at: Some("odrl:and".to_string()),
                    });
                }
            }
        }
        // The attribute is what the other two are *about*, so it is the one part
        // that cannot be missing. Either of the others alone is a complete
        // statement: a classification with no hierarchy publishes the column as
        // it is, which is what every policy written before the sixth term said.
        let Some(attribute) = attribute else {
            return Err(PolicyError::Missing(
                "an `odrl:and` naming a fossil:attribute",
            ));
        };
        if classification.is_none() && generalization.is_none() {
            return Err(PolicyError::Missing(
                "an `odrl:and` pairing fossil:attribute with fossil:classification or \
                 fossil:generalization",
            ));
        }
        if let Some(hierarchy) = generalization {
            let kind = hierarchy.get("kind").and_then(Value::as_str);
            if !kind.is_some_and(|k| profile::HIERARCHY_KINDS.contains(&k)) {
                return Err(PolicyError::UnknownHierarchyKind {
                    shape: shape.shape.clone(),
                    attribute,
                    kind: kind.map(ToString::to_string),
                });
            }
            shape.generalizations.push((attribute.clone(), hierarchy));
        }
        if let Some(classification) = classification {
            if classification == Classification::QuasiIdentifier {
                shape.quasi_identifiers.push(attribute.clone());
            }
            shape.classification.push((attribute, classification));
        }
        return Ok(());
    }

    let (left, right) = operand(constraint, expand)?;
    match left.as_str() {
        profile::ANONYMITY_K => *k = Some(as_u64(&right, "fossil:anonymityK")?),
        profile::SUPPRESSION_BUDGET => {
            *budget = Some(as_u64(&right, "fossil:suppressionBudget")?);
        }
        profile::ABSENT_QUASI_IDENTIFIER => {
            let word = as_str(&right, "fossil:absentQuasiIdentifier")?;
            *absent = Some(AbsentQuasiIdentifier::parse_word(word).ok_or_else(|| {
                PolicyError::UnknownTerm {
                    term: word.to_string(),
                    at: Some("fossil:absentQuasiIdentifier".to_string()),
                }
            })?);
        }
        // A bare `fossil:attribute` under a prohibition is the suppression rule:
        // this predicate is never published. Under a permission it is an
        // attribute named with nothing said about it, which is a document that
        // meant to say something.
        profile::ATTRIBUTE if prohibiting => {
            shape
                .prohibited
                .push(expand(as_str(&right, "rightOperand")?));
        }
        other => {
            return Err(PolicyError::UnknownTerm {
                term: other.to_string(),
                at: Some(shape.shape.clone()),
            });
        }
    }
    Ok(())
}

/// The `(leftOperand, rightOperand)` of one constraint, with the operand
/// expanded. `operator` is read and checked but carries no information the
/// reader acts on: `eq` and `gteq`/`lteq` are the only ones this profile's terms
/// admit, and each term admits exactly one, so an operator that disagrees with
/// its term is a document that means something this reader does not implement.
fn operand(
    constraint: &Value,
    expand: &impl Fn(&str) -> String,
) -> Result<(String, Value), PolicyError> {
    let left = constraint
        .get("leftOperand")
        .and_then(Value::as_str)
        .ok_or(PolicyError::Missing("leftOperand"))?;
    let operator = constraint
        .get("operator")
        .and_then(Value::as_str)
        .ok_or(PolicyError::Missing("operator"))?;
    let operator = operator.rsplit('/').next().unwrap_or(operator);
    let operator = operator.strip_prefix("odrl:").unwrap_or(operator);
    if !matches!(operator, "eq" | "gteq" | "lteq") {
        return Err(PolicyError::UnknownTerm {
            term: operator.to_string(),
            at: Some("odrl:operator".to_string()),
        });
    }
    let right = constraint
        .get("rightOperand")
        .cloned()
        .ok_or(PolicyError::Missing("rightOperand"))?;
    Ok((expand(left), right))
}

fn as_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, PolicyError> {
    v.as_str().ok_or_else(|| PolicyError::BadValue {
        key: key.to_string(),
        want: "a string",
        got: v.to_string(),
    })
}

fn as_u64(v: &Value, key: &str) -> Result<u64, PolicyError> {
    v.as_u64().ok_or_else(|| PolicyError::BadValue {
        key: key.to_string(),
        want: "a non-negative integer",
        got: v.to_string(),
    })
}

/// The prefix map from `@context`, having checked the ODRL context is there.
///
/// Only the local mapping object is read. Anything else in the array is another
/// vocabulary this reader does not implement, and a term it defines would be one
/// [`expand_term`] silently left compact.
fn context_prefixes(doc: &Value) -> Result<BTreeMap<String, String>, PolicyError> {
    let context = doc.get("@context").ok_or(PolicyError::ForeignContext)?;
    let entries: Vec<&Value> = match context {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    if !entries.iter().any(|e| e.as_str() == Some(ODRL_CONTEXT)) {
        return Err(PolicyError::ForeignContext);
    }
    let mut prefixes = BTreeMap::new();
    for entry in entries {
        if let Some(map) = entry.as_object() {
            for (prefix, iri) in map {
                if let Some(iri) = iri.as_str() {
                    prefixes.insert(prefix.clone(), iri.to_string());
                }
            }
        }
    }
    Ok(prefixes)
}

/// `fossil:attribute` → `https://fossil-lang.org/ns/privacy#attribute`, using
/// only the prefixes the document itself declared. A term with no colon, or with
/// a prefix the document did not declare, is returned unchanged — it is either
/// an ODRL core term or a bare column name, and both are legitimate here.
fn expand_term(term: &str, prefixes: &BTreeMap<String, String>) -> String {
    if term.starts_with("http://") || term.starts_with("https://") {
        return term.to_string();
    }
    match term.split_once(':') {
        Some((prefix, rest)) => prefixes
            .get(prefix)
            .map_or_else(|| term.to_string(), |iri| format!("{iri}{rest}")),
        None => term.to_string(),
    }
}
