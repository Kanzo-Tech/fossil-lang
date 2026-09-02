//! The privacy policy document a corpus is verified against.
//!
//! # Why this is a separate document and not an annotation on the shape
//!
//! A fossil program already binds one external document per output type:
//! `type { Person } := io.shex("persons.shex")`. The obvious place to put a
//! privacy classification is *in that document* — ShEx and SHACL both have
//! annotation mechanisms, and the shape already enumerates every predicate.
//!
//! **It must not go there, and the reason is reuse.** A shape says what a
//! `Person` is: which predicates it carries, with what cardinality and what
//! datatype. That is a property of the vocabulary, and it is the same in every
//! jurisdiction and every deployment. Whether `birth_year` is a
//! quasi-identifier is neither: it depends on what else is released, on who the
//! recipient is, and on which regulator is reading. Baking the second into the
//! first destroys the reuse of the first — one shape per deployment, differing
//! only in annotations, is how a vocabulary stops being shared.
//!
//! So it is a second document, bound the same way, and the two are joined by
//! the predicate IRI they both name.
//!
//! # The vocabulary, and why three of them
//!
//! - **ODRL 2.2 for structure.** `odrl:Policy`, `odrl:Prohibition`,
//!   `odrl:Constraint`, and the `leftOperand`/`operator`/`rightOperand` triple.
//!   This is not decoration: it is what makes a fossil policy readable by a
//!   data-space runtime that has never heard of fossil.
//! - **DPV for the classification values**, where DPV has a term.
//! - **A minimal own profile for the rest**, declared through `odrl:profile`.
//!   DPV names privacy *techniques* and has no idiom for their **parameters** —
//!   there is nowhere in it to put an ε or a k — and that gap is what the
//!   profile fills. It is deliberately tiny; see [`profile`].
//!
//! # What it can express
//!
//! Per-predicate classification, a shape-level quasi-identifier **set**, a
//! target `k`, a suppression budget, the null semantics, prohibition — "this
//! predicate is never published" — and, per predicate, the **generalisation
//! hierarchy** the writer reaches `k` with.
//!
//! # What it deliberately cannot
//!
//! It names no anonymisation **operator**, and the language has none either. An
//! obligation an author discharges by remembering to call a function is one they
//! can forget, and the failure is silent: the corpus writes, the manifest seals,
//! and the missing call is discovered by the recipient. The policy states the
//! bound; the writer refuses to seal a corpus that misses it.
//!
//! [`ShapeRule::generalizations`] is not a retreat from that and the distinction
//! is the whole of why it is here. A hierarchy is a **parameter**, in the same
//! sense `k` is: it is declared once, beside the bound it exists to reach, and
//! it is applied by the writer on every run whether or not anybody remembered
//! anything. There is still no `anon.generalize` to call and forget — there is
//! no call. What changed is that the bound became reachable: for as long as the
//! policy could name a `k` and no hierarchy, the verifier could only ever
//! refuse, and a corpus passed exactly when its source data happened to be
//! k-anonymous already. That is the half of a mechanism that exists without the
//! half that makes it usable, and it is not a stricter design than this one.

#![deny(unsafe_code)]

pub mod document;
pub mod profile;

pub use document::{PolicyError, parse};

use serde::{Deserialize, Serialize};

/// What one predicate is, for disclosure purposes.
///
/// ISO/IEC 20889:2018 is the definitional anchor rather than an invented one,
/// and **its terms are finer than these four**, in a way worth stating because
/// the obvious mapping is wrong.
///
/// The standard defines *direct identifier* (3.10) as an attribute that "alone
/// enables unique identification of a data principal within a specific
/// operational context", *indirect identifier* (3.16) as one that does so
/// "together with other attributes", and — separately — *quasi-identifier*
/// (3.28) as one that "when considered in conjunction with other attributes in
/// the dataset, singles out a data principal". **Quasi-identifier is not a
/// synonym for indirect identifier there.** They sit on different axes: the
/// identifier pair is about identification in an operational context, the
/// quasi-identifier is about singling out within the dataset, and Clause 6 draws
/// exactly that distinction. ISO's own synonym for *indirect identifier* is "key
/// attribute".
///
/// This enum is the dataset axis, because that is the axis a file can be checked
/// against: [`Self::QuasiIdentifier`] is 3.28's term used in 3.28's sense, and
/// [`Self::DirectIdentifier`] is 3.10's. The other axis needs an operational
/// context, which is not in the bytes.
///
/// "In conjunction with other attributes" is the entire reason
/// [`ShapeRule::quasi_identifiers`] is a **set** and not a per-predicate flag:
/// no single quasi-identifier has a k, and asking for one is a category error.
/// ISO 3.11 names the thing that does have one — an *equivalence class*, "set of
/// records in a dataset that have the same values for a specified subset of
/// attributes" — and 3.18's K-anonymity is a floor on its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Classification {
    /// Identifies a person on its own — a national identifier, an email
    /// address, a full name in a small population.
    DirectIdentifier,
    /// Singles out a record when taken together with the other attributes of
    /// the dataset, and not alone. The members of a shape's
    /// [`ShapeRule::quasi_identifiers`] are drawn from these.
    QuasiIdentifier,
    /// Not an identifier, and the thing an attacker wants to learn — a
    /// diagnosis, a salary, a verdict. **k-anonymity says nothing about it**,
    /// which is the standard criticism of k-anonymity and is answered by
    /// ℓ-diversity rather than by a bigger k. Recorded here so that a policy can
    /// say which column is the sensitive one, and so that the asymmetry in
    /// [`ShapeRule::quasi_identifiers`]' docblock is expressible.
    Sensitive,
    /// Carries no disclosure risk in this deployment.
    Public,
}

/// How a `NULL` in a quasi-identifier column is read when equivalence classes
/// are formed.
///
/// **Declared, never assumed.** SQL groups all `NULL`s together, which is one
/// answer; statistical disclosure control treats a missing quasi-identifier as
/// a wildcard matching every value, which is the other.
///
/// The field settles it by refusing to settle it. sdcMicro's `freqCalc` takes
/// `alpha`, "a numeric value between 0 and 1 specifying how much keys that
/// contain missing values (NAs) should contribute to the calculation of fk and
/// Fk", defaulting to 1, where "each *wildcard-match* would be counted while
/// for `alpha=0` keys with missing values would be basically ignored" — a
/// continuum with our [`Self::Wildcard`] at one end and something close to
/// [`Self::Suppress`] at the other. Three values rather than a continuum
/// because a corpus is either certified or it is not, and a fractional
/// contribution produces a fractional `k` that no guard can re-derive from the
/// bytes as an integer.
///
/// (`measure_risk` is often cited for the same parameter and should not be: its
/// documentation does not list `alpha` at all, it reaches `freqCalc` through
/// `...`, and the three places sdcMicro describes the default disagree with each
/// other. `freqCalc`'s wording is the one to quote.)
///
/// A checker that picks one of these silently has answered a question that
/// belonged to the producer.
///
/// # The ordering between them, which is what makes the choice checkable
///
/// [`Self::Value`] is the conservative end. Two `NULL`s are equal there, so a
/// class is an exact tuple match; and every record compatible with `r` under
/// [`Self::Wildcard`] is either in `r`'s exact class or in some other class, so
/// `f_wildcard(r) >= f_value(r)` for every `r`. **A corpus that passes under
/// `value` passes under `wildcard`.** The declaration therefore only ever
/// matters in one direction: a producer choosing `wildcard` claims the *weaker*
/// bound and says so on the artifact.
///
/// # What `value` is not conservative against
///
/// Cell suppression. A producer that blanked quasi-identifier cells to reach
/// `k` is rewarded twice by `value`: the blanks are a category, and a large
/// class of all-`NULL` rows clears any `k` while carrying no generalisation at
/// all. [`Self::Suppress`] is the answer, and it is why the null semantics and
/// the suppression budget are one decision rather than two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AbsentQuasiIdentifier {
    /// A `NULL` is a category. Classes are exact tuple matches, `NULL = NULL`.
    /// What SQL does, stated so that nobody has to infer it from the engine.
    #[default]
    Value,
    /// A `NULL` matches every value. Record `r`'s class is every record
    /// agreeing with it wherever both are non-`NULL`. The permissive end.
    Wildcard,
    /// A record with any `NULL` quasi-identifier is not certified by a class at
    /// all: it is counted as suppressed and charged to the budget. The only one
    /// of the three under which the suppressed count can be non-zero.
    Suppress,
}

impl AbsentQuasiIdentifier {
    /// The spelling used in the policy document and in the manifest. One
    /// function so the two cannot drift.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Wildcard => "wildcard",
            Self::Suppress => "suppress",
        }
    }

    /// Parse the spelling. `None` rather than a default, because a policy that
    /// misspells this parameter must be refused and not silently given the
    /// conservative reading — the producer would then be claiming a bound they
    /// did not ask for.
    #[must_use]
    pub fn parse_word(s: &str) -> Option<Self> {
        match s {
            "value" => Some(Self::Value),
            "wildcard" => Some(Self::Wildcard),
            "suppress" => Some(Self::Suppress),
            _ => None,
        }
    }
}

/// One shape's rules: what each of its predicates is, and which of them jointly
/// have to reach `k`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeRule {
    /// The shape this applies to, by the label the program binds it under —
    /// `Person`. The same label the corpus writes as a vertex type, which is
    /// what joins a policy to a corpus.
    pub shape: String,
    /// Per-predicate classification. A predicate is named by its IRI where the
    /// vocabulary has one and by its short name otherwise, and a match is tried
    /// against the IRI first: a non-RDF corpus has no predicate IRIs at all, and
    /// requiring one would make the policy unusable for exactly the corpora
    /// where the shape is least likely to exist.
    pub classification: Vec<(String, Classification)>,
    /// **The quasi-identifier set** — the combination that must jointly reach
    /// `k`.
    ///
    /// Separate from the classification, and not merely derived from it,
    /// because the two say different things. The classification says a
    /// predicate *can* contribute to re-identification; the set says which
    /// combination the bound is computed over. They usually coincide, and when
    /// they do the document may omit this and it is filled in from the
    /// classification. They come apart when a predicate is a quasi-identifier
    /// that this release does not publish, or publishes already generalised to
    /// the point where the policy author does not want it in the tuple.
    ///
    /// An explicit set must be a **subset** of the predicates classified
    /// [`Classification::QuasiIdentifier`]; a set naming something classified
    /// otherwise is a policy contradicting itself and is refused at parse time.
    ///
    /// # The asymmetry worth knowing about
    ///
    /// Verifying k needs these columns and **not** the sensitive one. So an
    /// auditor can check compliance without ever being entitled to the data the
    /// policy exists to protect. ℓ-diversity would not have this property — it
    /// is a statement about the distribution of the sensitive attribute inside
    /// each class, so checking it means reading it. That is a real cost of
    /// moving to ℓ-diversity and it is not usually counted.
    pub quasi_identifiers: Vec<String>,
    /// Predicates this policy prohibits publishing at all. An
    /// `odrl:Prohibition`, and the one rule here that is checked by looking at
    /// the *schema* rather than at the rows: if a column for one of these is in
    /// the corpus, the corpus is refused, whatever is in it.
    pub prohibited: Vec<String>,
    /// **How each quasi-identifier is generalised** — the declaration the
    /// writer derives from, keyed the way [`Self::classification`] is keyed
    /// (predicate IRI where there is one, short name otherwise).
    ///
    /// The value is the hierarchy, held as **opaque JSON**: the object
    /// `fossil_kanon::hierarchy::Hierarchy` deserialises from, checked here only
    /// for a `kind` this vocabulary admits. This crate is a leaf and does not
    /// link the anonymiser — see [`profile::GENERALIZATION`] for why the
    /// `rightOperand` is the hierarchy inline rather than a registry name or a
    /// fetchable IRI, and why the levels inside are somebody else's to validate.
    ///
    /// # A missing entry is not an error, and it is not a default either
    ///
    /// A quasi-identifier with no hierarchy is **published as it is**. That is
    /// the behaviour the corpus had before this field existed and it stays
    /// available on purpose: a predicate that is already coarse in the source —
    /// a region code, a decade — has nothing to generalise and inventing a
    /// hierarchy for it would publish a `*` where a usable value was safe.
    ///
    /// The failure mode this leaves open is real and is caught elsewhere: a
    /// producer who *meant* to generalise and mistyped the predicate gets no
    /// generalisation, and then gets a **refusal** from the verifier rather than
    /// a release, because nothing about the corpus improved. The bound is what
    /// notices, which is the same division of labour every other field here
    /// follows.
    ///
    /// Entries whose attribute is not classified [`Classification::QuasiIdentifier`]
    /// are refused at parse time: generalising a column the bound is not
    /// computed over damages the release and buys no anonymity, and a policy
    /// that asks for it has confused two of its own fields.
    pub generalizations: Vec<(String, serde_json::Value)>,
}

impl ShapeRule {
    /// The classification of one predicate, matched on IRI then on short name.
    /// [`Classification::Public`] for a predicate the policy does not mention —
    /// **and that is a decision, not a fallback**: a policy that had to
    /// enumerate every predicate would be a policy that silently stops covering
    /// a shape the day the shape gains a field, which is the failure mode of
    /// every allow-list that is not maintained. The refusal that catches the
    /// real case is [`Self::prohibited`], which is an explicit list of what may
    /// not be published, and the checker reports unmentioned predicates rather
    /// than assuming they were considered.
    #[must_use]
    pub fn classify(&self, iri: Option<&str>, name: &str) -> Classification {
        self.classification
            .iter()
            .find(|(key, _)| iri == Some(key.as_str()) || key == name)
            .map_or(Classification::Public, |(_, c)| *c)
    }

    /// Whether this predicate may not be published at all.
    #[must_use]
    pub fn is_prohibited(&self, iri: Option<&str>, name: &str) -> bool {
        self.prohibited
            .iter()
            .any(|key| iri == Some(key.as_str()) || key == name)
    }

    /// The generalisation hierarchy declared for one predicate, matched on IRI
    /// then on short name — the same two-step [`Self::classify`] uses, because a
    /// non-RDF corpus has no predicate IRIs and a policy for one has to be
    /// writable anyway.
    ///
    /// `None` means «publish this column as it is», which is a position and not
    /// an omission — see [`Self::generalizations`].
    #[must_use]
    pub fn hierarchy(&self, iri: Option<&str>, name: &str) -> Option<&serde_json::Value> {
        self.generalizations
            .iter()
            .find(|(key, _)| iri == Some(key.as_str()) || key == name)
            .map(|(_, h)| h)
    }
}

/// A parsed, validated privacy policy: what the corpus has to satisfy before it
/// is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyPolicy {
    /// The document's `odrl:uid`. Carried onto the manifest so that a recipient
    /// can name the policy a corpus was verified against — a name, never a
    /// location, because a corpus that carried its own policy is a corpus that
    /// can be handed on with the policy rewritten.
    pub uid: String,
    /// The `odrl:profile` IRI. A reader that does not recognise it should not
    /// try to read the `leftOperand`s, and one that does knows exactly which
    /// terms it is going to find.
    pub profile: String,
    /// The k every shape's quasi-identifier set must reach. Corpus-level: the
    /// bound is a property of the release, and a per-shape k would be four
    /// bounds a recipient has to combine by hand.
    pub k: u64,
    /// How a `NULL` quasi-identifier is read.
    pub absent_quasi_identifier: AbsentQuasiIdentifier,
    /// The suppression allowance, in parts per million of the released
    /// population. An integer, because the manifest that records the outcome is
    /// parsed by independent readers and a float is the one scalar they can
    /// disagree about over the same bytes.
    pub suppression_budget_ppm: u64,
    /// One entry per shape the policy covers. A shape the policy does not
    /// mention is **not** silently exempt: the verifier reports it, because a
    /// policy that covers three of four output types and says nothing about the
    /// fourth is the single most likely way for this whole mechanism to pass
    /// while protecting nothing.
    pub shapes: Vec<ShapeRule>,
}

impl PrivacyPolicy {
    /// The rule for one shape label, if the policy covers it.
    #[must_use]
    pub fn shape(&self, label: &str) -> Option<&ShapeRule> {
        self.shapes.iter().find(|s| s.shape == label)
    }
}
