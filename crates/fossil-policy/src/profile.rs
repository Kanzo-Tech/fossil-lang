//! The fossil privacy profile: five `odrl:LeftOperand`s and two classification
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
//! # Why five, and why it stops at five
//!
//! Catena-X is the one production European data-space profile and it ships
//! **four** `odrl:LeftOperand`s — `FrameworkAgreement`, `Membership`,
//! `ContractReference`, `UsagePurpose` — for an entire automotive industry, and
//! not one of them is schema-aware. That is the bar. Five here, of which one
//! ([`ATTRIBUTE`]) buys the granularity ODRL lacks and three are the parameters
//! DPV cannot hold. A sixth needs an argument this file does not have.
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

    /// Five left operands and no more. The number is the argument — Catena-X
    /// ships four for an industry — so it is asserted rather than described,
    /// and a sixth turns this red on the way in.
    #[test]
    fn the_profile_mints_five_left_operands() {
        let minted = [
            ATTRIBUTE,
            CLASSIFICATION,
            ANONYMITY_K,
            ABSENT_QUASI_IDENTIFIER,
            SUPPRESSION_BUDGET,
        ];
        assert_eq!(minted.len(), 5);
        for term in minted {
            assert!(term.starts_with(NAMESPACE), "{term} is not ours to mint");
        }
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
