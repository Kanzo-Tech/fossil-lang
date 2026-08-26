//! The policy reader, against documents built to be refused.
//!
//! Every case here is a thing the reader will not guess at. The reason there
//! are so many refusals and one acceptance is the asymmetry the module header
//! states: a reader that silently understands half a policy has dropped half a
//! policy, and the half it dropped may be the half that said "do not publish
//! this".

use fossil_policy::document::{PolicyError, parse};
use fossil_policy::{AbsentQuasiIdentifier, Classification};

/// A complete, valid policy — the one the documentation shows and the one every
/// case below mutates.
const PERSONS: &str = r#"{
  "@context": [
    "http://www.w3.org/ns/odrl.jsonld",
    { "fossil": "https://fossil-lang.org/ns/privacy#", "dpv": "https://w3id.org/dpv#" }
  ],
  "@type": "Set",
  "uid": "https://example.org/policies/persons-v1",
  "profile": "https://fossil-lang.org/ns/privacy/v1",
  "permission": [
    {
      "target": "Person",
      "action": "use",
      "constraint": [
        { "leftOperand": "fossil:anonymityK", "operator": "gteq", "rightOperand": 5 },
        { "leftOperand": "fossil:absentQuasiIdentifier", "operator": "eq", "rightOperand": "suppress" },
        { "leftOperand": "fossil:suppressionBudget", "operator": "lteq", "rightOperand": 20000 },
        { "and": [
          { "leftOperand": "fossil:attribute", "operator": "eq",
            "rightOperand": "https://example.org/birthYear" },
          { "leftOperand": "fossil:classification", "operator": "eq",
            "rightOperand": "fossil:QuasiIdentifier" } ] },
        { "and": [
          { "leftOperand": "fossil:attribute", "operator": "eq",
            "rightOperand": "https://example.org/postcode" },
          { "leftOperand": "fossil:classification", "operator": "eq",
            "rightOperand": "fossil:QuasiIdentifier" } ] },
        { "and": [
          { "leftOperand": "fossil:attribute", "operator": "eq",
            "rightOperand": "https://example.org/diagnosis" },
          { "leftOperand": "fossil:classification", "operator": "eq",
            "rightOperand": "dpv:SensitivePersonalData" } ] },
        { "and": [
          { "leftOperand": "fossil:attribute", "operator": "eq",
            "rightOperand": "https://example.org/email" },
          { "leftOperand": "fossil:classification", "operator": "eq",
            "rightOperand": "dpv:IdentifyingPersonalData" } ] }
      ]
    }
  ],
  "prohibition": [
    {
      "target": "Person",
      "action": "use",
      "constraint": [
        { "leftOperand": "fossil:attribute", "operator": "eq",
          "rightOperand": "https://example.org/nationalId" }
      ]
    }
  ]
}"#;

fn without(what: &str) -> String {
    PERSONS
        .lines()
        .filter(|l| !l.contains(what))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_document_the_documentation_shows_is_the_document_that_parses() {
    let policy = parse(PERSONS).expect("a valid policy");
    assert_eq!(policy.uid, "https://example.org/policies/persons-v1");
    assert_eq!(policy.k, 5);
    assert_eq!(
        policy.absent_quasi_identifier,
        AbsentQuasiIdentifier::Suppress
    );
    assert_eq!(policy.suppression_budget_ppm, 20_000);

    let person = policy.shape("Person").expect("the Person rule");
    // The set is the two quasi-identifiers and NOT the sensitive one, which is
    // the asymmetry the whole thing turns on: verifying k needs these two
    // columns and never `diagnosis`.
    assert_eq!(
        person.quasi_identifiers,
        vec![
            "https://example.org/birthYear".to_string(),
            "https://example.org/postcode".to_string(),
        ]
    );
    assert_eq!(
        person.prohibited,
        vec!["https://example.org/nationalId".to_string()]
    );
    assert_eq!(
        person.classify(Some("https://example.org/diagnosis"), "diagnosis"),
        Classification::Sensitive
    );
    assert_eq!(
        person.classify(Some("https://example.org/email"), "email"),
        Classification::DirectIdentifier
    );
    // A predicate the policy never mentions. `Public` is the answer and it is a
    // decision, not a fallback — see `ShapeRule::classify`.
    assert_eq!(person.classify(None, "colour"), Classification::Public);
}

/// ODRL §3.2: *"if the ODRL Processing system does not recognise the ODRL
/// Profile identifier(s) then it MUST stop processing the policy"*. Stopping is
/// the specified behaviour, not our caution.
#[test]
fn a_policy_written_against_another_profile_stops_the_reader() {
    let other = PERSONS.replace(
        "https://fossil-lang.org/ns/privacy/v1",
        "https://w3id.org/catenax/policy/profile2405",
    );
    match parse(&other).unwrap_err() {
        PolicyError::WrongProfile { found } => {
            assert_eq!(
                found.as_deref(),
                Some("https://w3id.org/catenax/policy/profile2405")
            );
        }
        other => panic!("{other:?}"),
    }
    // And no profile at all is the same refusal, because ODRL makes `profile`
    // mandatory once a policy uses one — a document with our terms and no
    // declaration is not a document that means them.
    assert!(matches!(
        parse(&without("\"profile\"")).unwrap_err(),
        PolicyError::WrongProfile { found: None }
    ));
}

/// The ODRL context is what makes a bare `"use"` mean `odrl:use` and a bare
/// `uid` mean `@id`. Without it the reader would be inventing those meanings.
#[test]
fn a_document_without_the_odrl_context_is_refused_rather_than_assumed() {
    let bare = PERSONS.replace("\"http://www.w3.org/ns/odrl.jsonld\",", "");
    assert_eq!(parse(&bare).unwrap_err(), PolicyError::ForeignContext);
}

#[test]
fn a_left_operand_outside_the_profile_is_refused() {
    let extra = PERSONS.replace("fossil:anonymityK", "fossil:differentialPrivacyEpsilon");
    match parse(&extra).unwrap_err() {
        PolicyError::UnknownTerm { term, .. } => {
            assert_eq!(
                term,
                "https://fossil-lang.org/ns/privacy#differentialPrivacyEpsilon"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// DPV has plenty of terms that are *about* personal data and are not one of
/// the four classifications. Accepting one because it looked close enough is
/// how a sensitive column ends up classified as something the checker ignores.
#[test]
fn a_classification_this_profile_does_not_define_is_refused() {
    let close = PERSONS.replace("dpv:SensitivePersonalData", "dpv:PersonalData");
    match parse(&close).unwrap_err() {
        PolicyError::UnknownTerm { term, at } => {
            assert_eq!(term, "https://w3id.org/dpv#PersonalData");
            assert_eq!(at.as_deref(), Some("fossil:classification"));
        }
        other => panic!("{other:?}"),
    }
}

/// The one parameter with no default. Both readings of an absent
/// quasi-identifier are defensible, so a document that does not choose has not
/// said what its own `k` means, and defaulting would put words in its mouth.
#[test]
fn a_policy_that_does_not_choose_a_null_reading_is_incomplete() {
    let silent = without("fossil:absentQuasiIdentifier");
    assert!(matches!(
        parse(&silent).unwrap_err(),
        PolicyError::Missing(_)
    ));
}

/// The budget DOES default, and to zero — the direction that cannot silently
/// weaken a bound. A policy saying nothing about suppression allows none.
#[test]
fn a_policy_silent_about_suppression_allows_none() {
    let silent = without("fossil:suppressionBudget");
    let policy = parse(&silent).expect("a policy without a budget is still a policy");
    assert_eq!(policy.suppression_budget_ppm, 0);
}

#[test]
fn a_policy_with_no_k_is_not_a_k_anonymity_policy() {
    assert!(matches!(
        parse(&without("fossil:anonymityK")).unwrap_err(),
        PolicyError::Missing(_)
    ));
}

/// A prefix the document did not declare is not expanded, so it cannot
/// accidentally resolve to one of ours. `fossil:` means what THIS document said
/// it means, and a document that forgot to say is a document whose terms are
/// bare strings.
#[test]
fn only_the_prefixes_the_document_declares_are_expanded() {
    let undeclared = PERSONS.replace(
        "{ \"fossil\": \"https://fossil-lang.org/ns/privacy#\", \"dpv\": \"https://w3id.org/dpv#\" }",
        "{ \"dpv\": \"https://w3id.org/dpv#\" }",
    );
    // The profile IRI is a full IRI and still resolves, so the reader gets past
    // that check — and then finds `fossil:anonymityK` unexpanded and unknown.
    match parse(&undeclared).unwrap_err() {
        PolicyError::UnknownTerm { term, .. } => assert_eq!(term, "fossil:anonymityK"),
        other => panic!("{other:?}"),
    }
}
