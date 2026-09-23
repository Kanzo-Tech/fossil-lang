//! The fossil privacy profile: six `odrl:LeftOperand`s and two classification
//! concepts, and the argument for each one's existence.
//!
//! # Why a profile at all
//!
//! ODRL 2.2 sanctions this explicitly. The Information Model's profile
//! mechanism (§3.3) says, in as many words, *"Additional Constraint left
//! operands: Create an instance of the `LeftOperand` class"*, and §3.2 makes
//! [`PROFILE`] mandatory once one is used: *"If an ODRL Policy conforms to an
//! ODRL Profile, then the `profile` property MUST be specified"*, and *"if the
//! ODRL Processing system does not recognise the ODRL Profile identifier(s) then
//! it MUST stop processing the policy"*. That last clause is the property worth
//! having: a runtime that does not know this profile is required to stop, not to
//! guess.
//!
//! # Why the terms below are not somebody else's
//!
//! Four vocabularies were read for each of them, and the gaps are real:
//!
//! - **ODRL has no granularity below the asset.** `odrl:target` names an
//!   `odrl:Asset` or an `odrl:AssetCollection` and there the vocabulary stops —
//!   no column, no attribute, no field, no schema. (`odrl:attribute` exists and
//!   is the *attribution* action, `includedIn odrl:use`; it is not this.) So
//!   [`ATTRIBUTE`] is unavoidable, and it is the term that does the most work
//!   here.
//! - **DPV names techniques and cannot parameterise them.** `dpv:Anonymisation`,
//!   `dpv:Pseudonymisation` and `dpv:DifferentialPrivacy` are all there as bare
//!   classes you assert or do not; there is no property in DPV whose range is a
//!   number. Not one `xsd:decimal`, `xsd:integer`, `xsd:float` or `xsd:double`
//!   appears in DPV core, `dpv-tech` or `dpv-risk`, and every `has*` property is
//!   concept-valued — `dpv:hasScale`'s range is `dpv:Scale`, an enumeration, not
//!   a magnitude. **There is nowhere in DPV to put an ε or a k.** [`ANONYMITY_K`],
//!   [`ABSENT_QUASI_IDENTIFIER`] and [`SUPPRESSION_BUDGET`] are that gap.
//! - **DPV has no quasi-identifier.** Case-insensitive search across all eight
//!   DPV 2.3 serializations for `QuasiIdentifier`, `IndirectIdentifier` or
//!   `DirectIdentifier` returns nothing. It has `dpv:IdentifyingPersonalData`
//!   ("Personal Data that explicitly and by itself is sufficient to identify a
//!   person"), which is a direct identifier and is used below; the *combination*
//!   case is absent. Hence [`QUASI_IDENTIFIER`].
//!
//! # Why six, and the argument the sixth needed
//!
//! Catena-X is the one production European data-space profile and it ships
//! **four** `odrl:LeftOperand`s — `FrameworkAgreement`, `Membership`,
//! `ContractReference`, `UsagePurpose` — for an entire automotive industry, and
//! not one of them is schema-aware. That is the bar. One ([`ATTRIBUTE`]) buys
//! the granularity ODRL lacks, three are the parameters DPV cannot hold, and
//! this file said for as long as there were five of them that **a sixth needs
//! an argument it did not have**. Here is the argument.
//!
//! The five terms let a policy *state* a bound. Not one of them lets a writer
//! *reach* one. That was not a gap in the vocabulary, it was a gap in the
//! system: `fossil-kanon` could derive a generalisation and `fossil-df` could
//! verify one, and nothing anywhere said which hierarchy a given predicate is
//! generalised by — so the verification could only ever refuse, and a release
//! passed only if the source data happened to be k-anonymous already. A bound
//! nothing can satisfy is not a bound, it is a refusal with a number on it.
//! [`GENERALIZATION`] is that missing edge and it is the whole of it: one term,
//! carrying one hierarchy, for one attribute.
//!
//! Catena-X's four are the wrong bar for *this* term specifically, and it is
//! worth saying why rather than treating «four is the bar» as arithmetic. All
//! four are about contract eligibility — who may use the asset, under which
//! agreement, for what stated purpose. None is schema-aware, so none of them
//! could parameterise a transformation even in principle; a profile that
//! reaches inside the asset is answering a question that profile never asks.
//! The bar it does set — *do not mint a term for something an existing
//! vocabulary can say* — is the one applied here, and it is met: see the DPV
//! paragraph above, which holds for a hierarchy exactly as it holds for a k.
//! DPV has `dpv:Generalisation` as a bare class you assert or do not, and
//! nowhere to put the levels.
//!
//! A **seventh** now needs an argument this file does not have. The one that
//! will be asked for first is a generalisation *margin* — «derive to k times
//! this» — as a mitigation for the minimality attack, and the answer is that
//! [`ANONYMITY_K`] already expresses it: a producer who wants a table that is
//! not minimal for `k` declares a larger `k` and gets exactly that, with the
//! number they chose published on the artifact instead of hidden in a
//! multiplier. See `fossil_df::generalize` for what the writer does about
//! minimality with the terms that exist.
//!
//! # The interoperability claim, which is weaker than it looks
//!
//! The reason usually given for minting into ODRL rather than inventing a
//! format is that the Dataspace Protocol leaves `leftOperand` open. **It does
//! not.** DSP's `contract-schema.json` pins `LeftOperand` to a closed `enum` of
//! exactly the 34 ODRL Core instances, with no extension slot, no `pattern` and
//! no `anyOf` — a policy carrying [`ATTRIBUTE`] fails that schema. What is true
//! is narrower and still worth having:
//!
//! - DSP's SHACL shapes constrain `odrl:target`, `dspace:timestamp`,
//!   `odrl:assigner` and `odrl:assignee`, and place **no** constraint on
//!   `leftOperand`; the prose specification says nothing about it either. The
//!   closed enum is one artefact of three, and the same file has `odrl:term-lteq`
//!   in its `Operator` enum — an HTML anchor pasted in place of `odrl:lteq` —
//!   which is a fair indication of how much load it is bearing.
//! - Eclipse EDC shipped the enum as a Java type, had it filed as *"a severe
//!   blocker"* because the model *"cannot express constraints other than those
//!   defined in the enum class"*, and moved to free-form.
//!
//! So the honest statement is: this is an ODRL profile in ODRL's own sense, and
//! it is readable by anything that reads ODRL, and a **strict** DSP validator
//! will reject it until DSP's schema catches up with ODRL's extension point.
//! That is a defect in one JSON Schema, not a reason to invent a fifth format.

/// The profile IRI. Emitted as `odrl:profile`, recorded on the manifest, and the
/// thing a reader checks before trying to interpret anything below.
pub const PROFILE: &str = "https://fossil-lang.org/ns/privacy/v1";

/// The namespace the terms below live in.
pub const NAMESPACE: &str = "https://fossil-lang.org/ns/privacy#";

/// `fossil:attribute` — which predicate a constraint is about.
///
/// The term ODRL has no equivalent for at any granularity: its `odrl:target` is
/// an asset, and a corpus column is not one. The `rightOperand` is the predicate
/// IRI, or the short column name for a corpus whose vocabulary has no IRIs.
pub const ATTRIBUTE: &str = "https://fossil-lang.org/ns/privacy#attribute";

/// `fossil:classification` — what that predicate is, for disclosure purposes.
///
/// The `rightOperand` is one of [`classification`]'s four values, two of them
/// DPV's and two of them ours because DPV has none.
pub const CLASSIFICATION: &str = "https://fossil-lang.org/ns/privacy#classification";

/// `fossil:anonymityK` — the k every equivalence class must reach.
pub const ANONYMITY_K: &str = "https://fossil-lang.org/ns/privacy#anonymityK";

/// `fossil:absentQuasiIdentifier` — how a missing quasi-identifier is read.
/// `value`, `wildcard` or `suppress`.
pub const ABSENT_QUASI_IDENTIFIER: &str =
    "https://fossil-lang.org/ns/privacy#absentQuasiIdentifier";

/// `fossil:suppressionBudget` — the suppression allowance, in parts per million
/// of the released population.
pub const SUPPRESSION_BUDGET: &str = "https://fossil-lang.org/ns/privacy#suppressionBudget";

/// `fossil:generalization` — **how** that predicate is generalised to reach
/// [`ANONYMITY_K`].
///
/// Paired with [`ATTRIBUTE`] under an `odrl:and`, exactly as [`CLASSIFICATION`]
/// is, and for the same reason: «this attribute is generalised by that
/// hierarchy» is two facts and a `Constraint` holds one triple.
///
/// # The `rightOperand` is the hierarchy itself, inline
///
/// Not a name drawn from a registry, and not an IRI pointing at a document.
///
/// A **registry** — `fossil:ukPostcode` and two others — would undo the reason
/// `fossil-kanon` ships its hierarchies as files in a directory rather than as
/// constants in a module: a postcode hierarchy is a fact about a country's
/// postcode system, revised by statisticians and not by programmers, and a
/// closed enumeration in this file would make a recompile the unit of change
/// for it again. Amnesia ships files; the directory is meant to be the
/// interface.
///
/// An **IRI to fetch** is the one thing `document.rs` refuses on principle. A
/// compile-time document whose meaning depends on a host being up is not a
/// document, it is an outage, and that argument does not weaken because the
/// thing fetched is a hierarchy rather than a `@context`.
///
/// So it is the object `fossil-kanon`'s `Hierarchy` deserialises from, written
/// out in place — `{"kind": "prefix", "lengths": [1, 2, 4, 6]}` — which means a
/// policy author pastes one of that crate's `hierarchies/*.json` files in
/// verbatim and it works, which is what «the directory is the interface» has to
/// mean if it means anything.
///
/// # And this crate does not interpret it
///
/// [`crate::ShapeRule::generalizations`] holds it as opaque JSON. `fossil-policy`
/// is a leaf — serde, `serde_json`, thiserror — and depending on the
/// anonymiser to read its own policy documents would invert the layering the
/// crate docs open with: the manifest names policy vocabulary, the policy names
/// nothing. What is checked here is that `kind` is one of the three the
/// vocabulary admits, so a typo is a policy error rather than a write-time one;
/// the levels inside are checked by the crate that can act on them.
pub const GENERALIZATION: &str = "https://fossil-lang.org/ns/privacy#generalization";

/// The three `kind`s a [`GENERALIZATION`] hierarchy may declare.
///
/// Mirrors `fossil_kanon::hierarchy::Hierarchy`'s serde tag, and is checked
/// against rather than converted to — see [`GENERALIZATION`] on why this crate
/// does not link the anonymiser. A fourth kind there is a fourth entry here and
/// `crates/fossil-df/tests/generalize.rs` is where the two are made to meet.
pub const HIERARCHY_KINDS: [&str; 3] = ["numeric", "prefix", "date"];

/// `dpv:IdentifyingPersonalData` — DPV's own, and an exact fit: *"Personal Data
/// that explicitly and by itself is sufficient to identify a person"*.
pub const DIRECT_IDENTIFIER: &str = "https://w3id.org/dpv#IdentifyingPersonalData";

/// `fossil:QuasiIdentifier` — **minted, because DPV has no term for it.** The
/// combination case is absent from DPV 2.3 entirely; ISO/IEC 20889:2018 §3.28 is
/// the definition this stands for.
pub const QUASI_IDENTIFIER: &str = "https://fossil-lang.org/ns/privacy#QuasiIdentifier";

/// `dpv:SensitivePersonalData` — DPV's own.
pub const SENSITIVE: &str = "https://w3id.org/dpv#SensitivePersonalData";

/// `fossil:NoDisclosureRisk` — **minted.** DPV models personal data and its
/// sensitivity, not the judgement "this column carries no disclosure risk *in
/// this deployment*", which is the thing being said here and is a property of
/// the release rather than of the data.
pub const NO_DISCLOSURE_RISK: &str = "https://fossil-lang.org/ns/privacy#NoDisclosureRisk";

/// The `rightOperand` IRI for one classification, and back.
pub mod classification {
    use crate::Classification;

    /// The IRI a classification is written as.
    #[must_use]
    pub const fn iri(c: Classification) -> &'static str {
        match c {
            Classification::DirectIdentifier => super::DIRECT_IDENTIFIER,
            Classification::QuasiIdentifier => super::QUASI_IDENTIFIER,
            Classification::Sensitive => super::SENSITIVE,
            Classification::Public => super::NO_DISCLOSURE_RISK,
        }
    }

    /// The classification an IRI names, or `None`.
    ///
    /// `None` and not a default: a policy naming a classification this profile
    /// does not define is a policy written against a vocabulary this reader does
    /// not have, and ODRL §3.2 says what to do about that — stop.
    #[must_use]
    pub fn of(iri: &str) -> Option<Classification> {
        match iri {
            super::DIRECT_IDENTIFIER => Some(Classification::DirectIdentifier),
            super::QUASI_IDENTIFIER => Some(Classification::QuasiIdentifier),
            super::SENSITIVE => Some(Classification::Sensitive),
            super::NO_DISCLOSURE_RISK => Some(Classification::Public),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Classification;

    /// Six left operands and no more. The number is the argument — Catena-X
    /// ships four for an industry — so it is asserted rather than described,
    /// and a **seventh** turns this red on the way in.
    ///
    /// It was five, and the sixth ([`GENERALIZATION`]) arrived with the
    /// argument the module header now carries: the five stated a bound and none
    /// of them could reach one, so the verification could only refuse. Turning
    /// this test red is the point at which that argument has to be written
    /// down, and it is the reason the assertion is a literal count rather than
    /// `minted.len()` compared against itself.
    #[test]
    fn the_profile_mints_six_left_operands() {
        let minted = [
            ATTRIBUTE,
            CLASSIFICATION,
            ANONYMITY_K,
            ABSENT_QUASI_IDENTIFIER,
            SUPPRESSION_BUDGET,
            GENERALIZATION,
        ];
        assert_eq!(minted.len(), 6);
        for term in minted {
            assert!(term.starts_with(NAMESPACE), "{term} is not ours to mint");
        }
    }

    /// The hierarchy kinds this crate checks against are the ones the
    /// anonymiser deserialises, and the two lists live in different crates
    /// because `fossil-policy` does not link `fossil-kanon`. Here the list is
    /// merely pinned; `crates/fossil-df/tests/generalize.rs` is where a kind
    /// this crate admits is actually round-tripped through the crate that acts
    /// on it, which is the only place the two CAN be made to meet.
    #[test]
    fn the_admitted_hierarchy_kinds_are_the_three_the_anonymiser_has() {
        assert_eq!(HIERARCHY_KINDS, ["numeric", "prefix", "date"]);
    }

    /// Two of the four classification values are DPV's, and the two that are
    /// ours are ours because DPV has nothing to borrow. Asserted by namespace so
    /// that "we minted a term DPV already had" is a test failure.
    #[test]
    fn only_the_terms_dpv_lacks_are_minted() {
        assert!(DIRECT_IDENTIFIER.starts_with("https://w3id.org/dpv#"));
        assert!(SENSITIVE.starts_with("https://w3id.org/dpv#"));
        assert!(QUASI_IDENTIFIER.starts_with(NAMESPACE));
        assert!(NO_DISCLOSURE_RISK.starts_with(NAMESPACE));
    }

    #[test]
    fn every_classification_round_trips_through_its_iri() {
        for c in [
            Classification::DirectIdentifier,
            Classification::QuasiIdentifier,
            Classification::Sensitive,
            Classification::Public,
        ] {
            assert_eq!(classification::of(classification::iri(c)), Some(c));
        }
        assert_eq!(
            classification::of("https://w3id.org/dpv#PersonalData"),
            None
        );
    }
}
