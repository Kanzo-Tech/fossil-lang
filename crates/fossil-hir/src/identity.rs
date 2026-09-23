//! The identity of a TYPE — one `@subject` template per shape, for the whole
//! file.
//!
//! # Why this exists, and why it is file-keyed
//!
//! The identity is **unique per type**: every mapping that produces `T`
//! declares the same `@subject`, and disagreement is an error. That rule is
//! what turns an edge from a guess into a lookup — `buyer =
//! Person(User.email)` means «the `Person` whose identity is built from this
//! email», and there is only something to build if `Person` has ONE template.
//!
//! `crate::body::check_identity` enforces the other two obligations (present,
//! exactly one, first) and cannot enforce this one: it is keyed by
//! [`MappingLoc`], so by construction it cannot see a second mapping. Uniqueness
//! is a fact about a FILE, and this is the file-keyed query that holds the
//! table it is a fact about.
//!
//! # Two queries, and the split is about accumulators
//!
//! `subject_templates` holds the table and emits NOTHING. `check_identities`
//! reads that table and emits the disagreement diagnostics. They are separate
//! because salsa collects an accumulator over a query's whole dependency
//! subtree: `subject_templates` is read from inside `typecheck_mapping` and
//! `lower_to_mir_pg` (an edge constructor needs it), so a diagnostic pushed
//! there comes back out of EVERY mapping that constructs an edge — once per
//! mapping, for a fact about the file. `fossil_cli::check` would dedup it and
//! the LSP, which drains per mapping and does not, would show it N times.
//!
//! So the check is its own file-keyed query that nothing per-mapping reads, and
//! hosts drain it beside `def_map` and `lower_to_hir` — the two other file-level
//! drains that exist for exactly this reason.
//!
//! Xiao et al. (ESWC 2018) ASSUME the property in as many words — *«it is
//! desired that each IRI can be constructed by at most one IRI template, and we
//! make such assumption here»*. The difference is that fossil compiles the whole
//! program at once, so for us the assumption is checkable rather than
//! structural — and an unchecked assumption about identity ends as two entities
//! where there was one.
//!
//! # Fan-out
//!
//! Both queries read `body(db, m)` for every mapping in the file, so a body edit
//! re-executes them. They stay inside `MAX_PER_MAPPING_FAN_OUT = 1` because of
//! three things together:
//!
//! 1. They are **file-keyed**, so each re-executes ONCE, not once per mapping —
//!    the same property that made the shape document free (`shape_document` is
//!    keyed by the DOCUMENT, so ten mappings share one decode).
//! 2. `subject_templates`' per-mapping callers read it **lazily** — only a body
//!    that actually contains an [`HirExpr::Edge`] asks for it.
//! 3. Nothing per-mapping reads `check_identities` AT ALL. It is a host query,
//!    so it adds no edge to the per-mapping graph and cannot widen it.
//!
//! The sibling bodies they read do not re-execute: `mapping_cst_node` is the
//! barrier and their green subtrees are structurally equal. And
//! neither query reads `parse(db, file)`, which is what keeps their OUTPUT
//! stable across an edit that shifts byte offsets — see `SubjectTemplate::span`
//! for the trap that decides it.

use fossil_base::{SourceFile, Span};
use smol_str::SmolStr;

use crate::body::body;
use crate::def_map::def_map;
use crate::lower::{HirExpr, InterpolationPart, PropertyKey, lower_to_hir};

/// One type's identity template.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SubjectTemplate {
    /// The shape IRI the template builds identities for.
    pub shape_iri: SmolStr,
    /// The `@subject` expression itself, verbatim from the mapping's body.
    ///
    /// Usually an [`HirExpr::Interpolation`], which is the only form with holes
    /// to fill; a constant identity (no holes) is legal and takes zero
    /// arguments.
    pub template: HirExpr,
    /// Where it was written — the **mapping-relative** span of the `@subject`
    /// right-hand side. Carried so a second, disagreeing template can be blamed
    /// against the first, and so an arity error can point at the definition as
    /// well as at the call.
    ///
    /// # It is mapping-relative on purpose, and the reader has to rebase it
    ///
    /// Every consumer of this field is in the same position: it holds a span
    /// belonging to a mapping that is NOT the one it is reporting about, and
    /// `fossil_base::SpanFrame::MappingRelative` offsets from another mapping
    /// are meaningless in yours. [`Self::mapping_index`] is what closes that —
    /// `crate::spans::mapping_start_offset` turns the pair into a file-absolute
    /// span, and the label carries `SpanFrame::FileAbsolute` from then on.
    ///
    /// Storing it file-absolute here instead would be simpler to read and would
    /// cost the fan-out invariant: a file-absolute offset moves whenever an
    /// earlier edit changes the file's length, so `subject_templates`' OUTPUT
    /// would change on every keystroke, and every `typecheck_mapping` that reads
    /// it would re-execute. Mapping-relative offsets are stable under a sibling
    /// edit, which is the whole reason `crate::spans` is in that frame.
    pub span: Span,
    /// Which mapping declared it — its dense index among the file's MAPPINGs,
    /// i.e. `def_map(db, file).mappings(db)[i]`. The other half of [`Self::span`].
    pub mapping_index: usize,
    /// The name of the mapping that declared it, for the same two messages.
    pub mapping_name: SmolStr,
}

impl SubjectTemplate {
    /// How many holes the template has — the number of arguments the
    /// constructor takes.
    ///
    /// Holes are counted in ORDER OF APPEARANCE and that order is the binding
    /// order (see [`HirExpr::Edge`]). A template with no holes is a constant
    /// IRI: legal, and its constructor takes none.
    #[must_use]
    pub fn arity(&self) -> usize {
        match &self.template {
            HirExpr::Interpolation(parts) => parts
                .iter()
                .filter(|p| matches!(p, InterpolationPart::Hole(_)))
                .count(),
            _ => 0,
        }
    }

    /// The template with its holes replaced by `args`, left to right.
    ///
    /// Each argument is the hole's **finished value**, not an input to whatever
    /// the defining mapping wrote there: `Person(User.email)` yields
    /// `"https://shop.example/user/" + User.email` and never re-applies the
    /// definition's own expression to it. Two reasons, and the second is the
    /// one that decides it:
    ///
    /// - The hole's expression is written in the DEFINING mapping's scope.
    ///   `@subject = "…/{User.email}"` in a mapping over `Adults` names a row
    ///   the referencing mapping may not have; re-applying it would resolve
    ///   `User` in the wrong scope, which is either a wrong column or a
    ///   diagnostic about a row the author never mentioned.
    /// - It would make the hole's spelling public API. Renaming a column inside
    ///   `{…}` would change what every referencing call site has to pass.
    ///
    /// Returns `None` if `args.len()` does not match [`Self::arity`] — the
    /// caller reports it, because only the caller has the span of the call.
    #[must_use]
    pub fn fill(&self, args: &[HirExpr]) -> Option<HirExpr> {
        if args.len() != self.arity() {
            return None;
        }
        let HirExpr::Interpolation(parts) = &self.template else {
            // No holes: a constant identity. `arity()` is 0, so `args` is empty
            // and the template stands as written.
            return Some(self.template.clone());
        };
        let mut next = args.iter();
        let filled = parts
            .iter()
            .map(|part| match part {
                InterpolationPart::Text(t) => InterpolationPart::Text(t.clone()),
                InterpolationPart::Hole(_) => InterpolationPart::Hole(
                    next.next()
                        .cloned()
                        // Unreachable: the arity check above counted these.
                        .unwrap_or_else(|| HirExpr::StringLit(SmolStr::default())),
                ),
            })
            .collect();
        Some(HirExpr::Interpolation(filled))
    }
}

/// Every identity written in the file — **one entry per MAPPING**, in mapping
/// order, not one per shape.
///
/// It held one entry per shape until the disagreement check needed the others:
/// a table that keeps only the first template for `Person` cannot report that
/// the second one differs, because it never read it. Deduplication moved to
/// [`SubjectTemplates::for_shape`], where it belongs — that is the question
/// «what is THE identity of this type», and it has one answer precisely because
/// `check_identities` refuses the files where it would not.
#[salsa::tracked(debug)]
pub struct SubjectTemplates<'db> {
    #[returns(ref)]
    pub templates: Vec<SubjectTemplate>,
}

impl<'db> SubjectTemplates<'db> {
    /// THE template for a shape, or `None` when no mapping in the file produces
    /// it.
    ///
    /// «The» rather than «a» because a type has ONE identity: `None` here means
    /// an edge points at a type this program does not write, and that is **not
    /// an error** — an edge is a REFERENCE and RDF is open-world, so a `buyer`
    /// may name a `Person` no mapping emitted. What it does mean is that there
    /// is no template to fill, so the constructor
    /// cannot be used; the caller says that, with the span of the call.
    ///
    /// When several mappings produce the shape it answers with the FIRST, which
    /// is exactly right when they agree and is the reason `check_identities`
    /// exists for when they do not: an edge built from the first while the
    /// second mints something else reaches a node the second never wrote.
    #[must_use]
    pub fn for_shape(
        self,
        db: &'db dyn fossil_base::Db,
        shape_iri: &str,
    ) -> Option<SubjectTemplate> {
        self.templates(db)
            .iter()
            .find(|t| t.shape_iri == shape_iri)
            .cloned()
    }
}

/// The [`SubjectTemplates`] of one file.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn subject_templates<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
) -> SubjectTemplates<'db> {
    let dm = def_map(db, file);
    let hir = lower_to_hir(db, file);
    let mut templates: Vec<SubjectTemplate> = Vec::new();

    for (i, loc) in dm.mappings(db).iter().enumerate() {
        // The header carries the shape; a header that did not lower has already
        // been diagnosed by `lower_mapping_node` and contributes no identity.
        //
        // `HirFile::mapping` and not `.mappings(db).get(i)`, and the difference
        // was a wrong answer rather than a missing one: the vector used to skip
        // the mappings that declined, so `get(i)` handed mapping `i` the
        // signature of whichever mapping came after it. It is one slot per
        // `MAPPING` node now, so `None` here means THIS header declined.
        let Some(m) = hir.mapping(db, i) else {
            continue;
        };
        if m.shape_iri.is_empty() {
            continue;
        }
        let hir_body = body(db, *loc);
        let Some((idx, prop)) = hir_body
            .properties(db)
            .iter()
            .enumerate()
            .find(|(_, p)| matches!(p.key, PropertyKey::Subject))
        else {
            // No identity. `body::check_identity` has already said so, with the
            // mapping's name in the message; adding a second voice here would
            // report the same fact twice.
            continue;
        };
        let span = crate::spans::spans(db, *loc)
            .get(
                db,
                crate::body::ExprId(u32::try_from(idx).unwrap_or(u32::MAX)),
            )
            .unwrap_or(Span { start: 0, end: 0 });
        templates.push(SubjectTemplate {
            shape_iri: m.shape_iri.clone(),
            template: prop.value.clone(),
            span,
            mapping_index: i,
            mapping_name: m.name.clone(),
        });
    }

    SubjectTemplates::new(db, templates)
}

/// **One identity per type, and the file is what knows.**
///
/// Every mapping that produces `T` must declare the same `@subject`; two that
/// disagree are an ERROR naming both mappings and both templates. It may not be
/// a warning: a warning about identity is ignored, and the result is two
/// entities where there was one. Returns how many disagreements
/// it reported, so a test can assert the count without draining the accumulator;
/// the diagnostics themselves are what callers want.
///
/// # Nothing per-mapping may call this
///
/// It accumulates. See the module docs: a diagnostic pushed from a query that
/// `typecheck_mapping` reads comes back out once per mapping. Hosts drain it
/// once per FILE, beside `def_map` and `lower_to_hir`.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn check_identities(db: &dyn fossil_base::Db, file: SourceFile) -> usize {
    use salsa::Accumulator as _;

    let dm = def_map(db, file);
    let table = subject_templates(db, file);
    let templates = table.templates(db);

    let mut reported = 0usize;
    // The FIRST mapping to produce a shape is the reference every later one is
    // read against, so three disagreeing mappings give two diagnostics and each
    // names the definition — rather than a chain where the second is blamed
    // against the first and the third against the second.
    for (i, later) in templates.iter().enumerate() {
        let Some(first) = templates[..i]
            .iter()
            .find(|t| t.shape_iri == later.shape_iri)
        else {
            continue;
        };
        if identity_form(&first.template) == identity_form(&later.template) {
            continue;
        }
        reported += 1;

        // The type's name as the PROGRAM wrote it — `Person`, not
        // `https://shop.example/voc#Person`. A message about a mapping header
        // has to use the vocabulary of the mapping header; the IRI is the
        // compiler's key, not the author's word.
        let type_name = dm
            .types(db)
            .iter()
            .find(|t| t.shape_iri.as_deref() == Some(later.shape_iri.as_str()))
            .map_or_else(
                || SmolStr::from(fossil_graph_schema::local_name(&later.shape_iri)),
                |t| t.name.clone(),
            );

        // Both spans are rebased HERE, and that is the whole of the two-mapping
        // span problem: each template's `span` is relative to ITS OWN mapping,
        // and this query belongs to neither. `mapping_start_offset` is the
        // documented escape — a diagnostic-emission layer sitting outside the
        // per-mapping barrier — and once rebased the diagnostic and its label
        // both declare `FileAbsolute`, so no host rebases them a second time.
        let absolute = |t: &SubjectTemplate| -> Span {
            let base = dm
                .mappings(db)
                .get(t.mapping_index)
                .map_or(0, |loc| crate::spans::mapping_start_offset(db, *loc));
            Span::new(
                t.span.start.saturating_add(base),
                t.span.end.saturating_add(base),
            )
        };

        fossil_base::Diagnostic::new(
            fossil_base::Severity::Error,
            format!(
                "`{}` and `{}` mint two identities for {type_name}",
                first.mapping_name, later.mapping_name
            ),
            absolute(later),
        )
        .file_absolute()
        .with_label(
            absolute(first),
            format!("`{}` mints this one", first.mapping_name),
            fossil_base::SpanFrame::FileAbsolute,
        )
        .with_label(
            absolute(later),
            format!("`{}` mints this one", later.mapping_name),
            fossil_base::SpanFrame::FileAbsolute,
        )
        .with_help(format!(
            "a type has one identity. Every mapping that produces {type_name} writes the same \
             @subject, so that an edge naming {type_name} reaches the same node whichever \
             mapping emitted it."
        ))
        .accumulate(db);
    }
    reported
}

/// The form of an identity, for comparing two of them.
///
/// It is NOT the written text. `Users` reads `User.email` and `Imported` reads
/// `Legacy.email`; those are the same identity scheme written over two sources,
/// and the row binder is a name local to each mapping — the cost one identity
/// per type accepts is that two DIFFERENT identity schemes stop being
/// expressible, not that two sources for one type do. So a column reference
/// contributes its COLUMN and drops its binder, and the two agree.
///
/// What does not survive that erasure is the fixture this check exists for:
/// `{User.email}` against `{Legacy.account_id}` differ in the column, and
/// `.../user/{…}` against `.../u/{…}` differ in the literal text. Comparing
/// SKELETONS instead — every hole replaced by a marker, which is what
/// `fossil-mir`'s deleted `subject_skeletons` did — would call those two equal,
/// and it is exactly the program `apps/docs/programs/errors/two-identities`
/// exists to reject.
fn identity_form(e: &HirExpr) -> String {
    match e {
        // The binder is erased and the column kept. `FieldRef` is the retired
        // anonymous row — the leading dot died in favour of the qualified
        // `User.email` — and it normalises to the same thing, so a file still
        // carrying the old spelling does not report a disagreement between two
        // spellings of one reference.
        HirExpr::ColumnRef { column, .. } => format!(".{column}"),
        HirExpr::FieldRef(name) => format!(".{name}"),
        HirExpr::StringLit(s) => format!("{s:?}"),
        HirExpr::NullLit => "null".to_string(),
        HirExpr::Interpolation(parts) => {
            let body: String = parts
                .iter()
                .map(|p| match p {
                    InterpolationPart::Text(t) => t.replace('{', "{{"),
                    InterpolationPart::Hole(h) => format!("{{{}}}", identity_form(h)),
                })
                .collect();
            format!("\"{body}\"")
        }
        HirExpr::Call { func, args } => {
            let rendered: Vec<String> = args.iter().map(identity_form).collect();
            format!("{func}({})", rendered.join(", "))
        }
        HirExpr::Edge { target, args } => {
            let rendered: Vec<String> = args.iter().map(identity_form).collect();
            format!("{target}({})", rendered.join(", "))
        }
        HirExpr::IntLit(v) => v.to_string(),
        HirExpr::FloatLit(v) => format!("{v:?}"),
        HirExpr::BoolLit(b) => b.to_string(),
        HirExpr::UnaryOp { op, operand } => format!("({op:?} {})", identity_form(operand)),
        HirExpr::BinOp { op, lhs, rhs } => {
            format!("({} {op:?} {})", identity_form(lhs), identity_form(rhs))
        }
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => format!(
            "({} ? {} : {})",
            identity_form(cond),
            identity_form(then),
            identity_form(otherwise)
        ),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use fossil_base::test_support::{PERSON_DOCUMENT, db_with_document};

    /// The disagreement diagnostics alone.
    ///
    /// `check_identities::accumulated` yields its whole dependency subtree —
    /// every `body`, `lower_to_hir` and `def_map` diagnostic the file produced —
    /// which is why `fossil_cli::check` filters this drain by frame. These
    /// fixtures are clean, so the filter here is belt-and-braces: it keeps a
    /// failure in one of those readable as ITS failure rather than as this one.
    fn conflicts(db: &fossil_base::FossilDb, file: SourceFile) -> Vec<&fossil_base::Diagnostic> {
        check_identities::accumulated::<fossil_base::Diagnostic>(db, file)
            .into_iter()
            .filter(|d| d.message.contains("mint two identities"))
            .collect()
    }

    /// Two mappings over one shape, minting the same identity from two sources.
    /// The table is one entry per MAPPING, so both are in it;
    /// [`SubjectTemplates::for_shape`] is where the deduplication happens.
    const TWO_MAPPINGS: &str = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"u.csv\")
more  := io.csv(\"m.csv\")

A : Person from users
    @subject = \"http://example.org/p/{users.id}\"
    name = users.name

B : Person from more
    @subject = \"http://example.org/p/{more.id}\"
    name = more.name
";

    #[test]
    fn one_template_per_shape_not_per_mapping() {
        let (db, file) = db_with_document(TWO_MAPPINGS, "person.shex", PERSON_DOCUMENT);
        let table = subject_templates(&db, file);
        assert_eq!(
            table.templates(&db).len(),
            2,
            "the table is one entry per MAPPING — the second is what the \
             disagreement check reads, and dropping it is what made the check \
             impossible to write"
        );
        let t = table
            .for_shape(&db, "http://example.org/Person")
            .expect("the shape is produced by this file");
        assert_eq!(t.arity(), 1, "one hole, one constructor argument");
        assert_eq!(
            t.mapping_name, "A",
            "two mappings produce ONE type, so the type has one identity and it \
             is the first one written"
        );
        assert_eq!(
            check_identities(&db, file),
            0,
            "`{{users.id}}` and `{{more.id}}` are the same identity over two \
             sources: the row binder is local to each mapping"
        );
    }

    /// The fixture one identity per type exists for, in miniature: two
    /// mappings, one type, two different identities.
    /// `apps/docs/programs/errors/two-identities` is
    /// the same program in the conformance set, and it was ACCEPTED — the rule
    /// was written down in three places and enforced in none.
    const TWO_IDENTITIES: &str = "\
type { Person } := io.shex(\"person.shex\")
User := io.csv(\"u.csv\")
Legacy := io.csv(\"l.csv\")

Users : Person from User
    @subject = \"https://shop.example/user/{User.email}\"
    name = User.name

Imported : Person from Legacy
    @subject = \"https://shop.example/user/{Legacy.account_id}\"
    name = Legacy.full_name
";

    #[test]
    fn two_mappings_that_mint_different_identities_for_one_type_are_an_error() {
        let (db, file) = db_with_document(TWO_IDENTITIES, "person.shex", PERSON_DOCUMENT);
        assert_eq!(
            check_identities(&db, file),
            1,
            "one disagreement, one report"
        );

        let diags = conflicts(&db, file);
        let d = diags.first().expect("the disagreement is reported");
        assert_eq!(
            d.severity,
            fossil_base::Severity::Error,
            "a disagreement about identity is an error, not a warning: a warning \
             is ignored and the result is two entities where there was one"
        );
        assert_eq!(
            d.message, "`Users` and `Imported` mint two identities for Person",
            "the message names BOTH mappings and the type as the program spells it"
        );

        // Both templates are underlined, each with the name of the mapping that
        // wrote it, and both spans are FILE-absolute — one of them belongs to a
        // mapping this diagnostic is not about, and mapping-relative offsets
        // from another mapping land on a plausible, wrong line.
        let labels: Vec<&str> = d.labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            labels,
            vec!["`Users` mints this one", "`Imported` mints this one"]
        );
        assert!(
            d.labels
                .iter()
                .all(|l| l.frame == fossil_base::SpanFrame::FileAbsolute),
            "a label pointing into another mapping cannot be mapping-relative"
        );
        assert_eq!(d.frame, fossil_base::SpanFrame::FileAbsolute);

        for label in &d.labels {
            let text = &TWO_IDENTITIES[label.span.start as usize..label.span.end as usize];
            assert!(
                text.starts_with("\"https://shop.example/user/"),
                "the label underlines the template as written, and got {text:?}"
            );
        }
        assert_ne!(
            d.labels[0].span, d.labels[1].span,
            "two mappings, two places"
        );
    }

    /// The same two mappings agreeing: same literal runs, same column, two
    /// sources. It is not an error, and that is the rigidity one identity per
    /// type accepts and the rigidity it does not — two identity SCHEMES stop being
    /// expressible; two sources for one type do not.
    #[test]
    fn the_same_identity_over_two_sources_is_one_identity() {
        let agreeing = TWO_IDENTITIES.replace("Legacy.account_id", "Legacy.email");
        let (db, file) = db_with_document(&agreeing, "person.shex", PERSON_DOCUMENT);
        assert_eq!(check_identities(&db, file), 0);
        assert!(
            conflicts(&db, file).is_empty(),
            "`{{User.email}}` and `{{Legacy.email}}` are one identity written twice"
        );
    }

    /// Three mappings, three identities: each later one is read against the
    /// FIRST, so the report names the definition every time instead of chaining
    /// the second against the third.
    #[test]
    fn a_third_disagreement_is_blamed_against_the_definition() {
        let three = format!(
            "{TWO_IDENTITIES}\nThird : Person from User\n    \
             @subject = \"https://shop.example/person/{{User.email}}\"\n    name = User.name\n"
        );
        let (db, file) = db_with_document(&three, "person.shex", PERSON_DOCUMENT);
        assert_eq!(check_identities(&db, file), 2, "two later, two reports");
        let diags = conflicts(&db, file);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            vec![
                "`Users` and `Imported` mint two identities for Person",
                "`Users` and `Third` mint two identities for Person",
            ]
        );
    }

    /// The substitution keeps the literal runs and replaces only the holes, in
    /// order. This is the whole of «the argument is the hole's finished value».
    #[test]
    fn filling_replaces_the_holes_and_keeps_the_text() {
        let (db, file) = db_with_document(TWO_MAPPINGS, "person.shex", PERSON_DOCUMENT);
        let t = subject_templates(&db, file)
            .for_shape(&db, "http://example.org/Person")
            .expect("bound");
        let arg = HirExpr::ColumnRef {
            binding: "Order".into(),
            column: "buyer_id".into(),
        };
        let Some(HirExpr::Interpolation(parts)) = t.fill(std::slice::from_ref(&arg)) else {
            panic!("one argument for one hole must fill");
        };
        assert_eq!(
            parts,
            vec![
                InterpolationPart::Text("http://example.org/p/".into()),
                InterpolationPart::Hole(arg),
            ],
            "the defining mapping's `users.id` is GONE — it named a row the \
             referencing mapping does not have"
        );
    }

    /// Arity is checked by `fill` and reported by the caller, which is the only
    /// side that has the span of the call.
    #[test]
    fn the_wrong_number_of_arguments_fills_nothing() {
        let (db, file) = db_with_document(TWO_MAPPINGS, "person.shex", PERSON_DOCUMENT);
        let t = subject_templates(&db, file)
            .for_shape(&db, "http://example.org/Person")
            .expect("bound");
        assert!(t.fill(&[]).is_none(), "one hole, no arguments");
        assert!(
            t.fill(&[HirExpr::IntLit(1), HirExpr::IntLit(2)]).is_none(),
            "one hole, two arguments"
        );
    }
}
