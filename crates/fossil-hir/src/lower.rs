//! CST → HIR lowering.
//!
//! Phase 2 (plan 02-04) splits Phase 1's flat `HirMapping` shape along the
//! line the item tree draws — a signature is eager, a body is not:
//! header signature fields (name, shape IRI, source binding) live
//! here; the per-mapping property list moved into [`crate::body::HirBody`],
//! reached via the [`crate::body::body`] Salsa query keyed by
//! [`crate::def_map::MappingLoc`]. The split is the precondition for
//! CORE-02 SC#2: editing one property's right-hand side invalidates only
//! `body(M_k)` + its downstream queries, never the file-level
//! `item_tree(file)` or `lower_to_hir(file)` queries' structural inputs.
//!
//! The expression encoding remains intentionally minimal:
//! - [`HirExpr::Interpolation`] carries a string's literal runs and its holes,
//!   each hole an ordinary expression. It replaced a raw `Template` that kept
//!   backtick text verbatim for codegen to re-scan; there is no backtick.
//! - [`HirExpr::FieldRef`] is just the name of a column of the one row in scope.
//! - [`HirExpr::StringLit`] holds the literal text without surrounding quotes.

use fossil_base::{Diagnostic, Severity, SourceFile, Span};
use salsa::Accumulator;
use smol_str::SmolStr;

use crate::def_map::def_map;

#[salsa::tracked(debug)]
pub struct HirFile<'db> {
    #[returns(ref)]
    pub mappings: Vec<HirMapping>,
    /// The source bindings whose right-hand side is a PIPELINE rather than an
    /// `io.*` call. A binding that reads a file is `def_map`'s business — a
    /// constructor and a URI, both signature-only; a binding that derives a
    /// relation from another one carries expressions, and expressions are lowered
    /// here or they are lowered twice.
    #[returns(ref)]
    pub source_pipes: Vec<HirSourcePipe>,
}

/// Per-mapping HEADER data. The previous `properties` field
/// is REMOVED — body content lives in [`crate::body::HirBody`], reached via
/// [`crate::body::body`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirMapping {
    /// Mapping name, e.g. `"User"`.
    pub name: SmolStr,
    /// Fully-resolved shape IRI, e.g. `"https://example.org/Person"`.
    pub shape_iri: SmolStr,
    /// Name of the source binding referenced by `from`, e.g. `"users"`.
    pub source_binding: SmolStr,
}

/// `Adults := User.where(User.age >= 18)` — a source binding that is a
/// RELATION derived from another binding, not a file to read.
///
/// `base` is the binding at the head of the pipe; every stage after it is one
/// [`HirSourceOp`] in written order. The head must be a name and not another
/// call, because a pipeline whose head is `io.csv("u.csv")` would give the same
/// relation two spellings — and `from <name>` resolves bindings, not expressions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirSourcePipe {
    pub name: SmolStr,
    pub base: SmolStr,
    pub ops: Vec<HirSourceOp>,
    /// Start and end of the whole `name := ...` item, so the checker's row
    /// algebra has somewhere to point. One span for the pipeline and not one per
    /// stage: a wrong column is a fact about the pipeline, and a per-expression
    /// span belongs in the `Spans<'db>` side table keyed by
    /// `(MappingLoc, ExprId)`, never in a field of the HIR node itself.
    pub span: (u32, u32),
}

/// The three verbs of the first version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum HirSourceOp {
    /// `where(User.age >= 18)` — keeps the rows the predicate holds for. The row
    /// type is unchanged, which is why it is the cheap one.
    Where(HirExpr),
    /// `select(User.id, User.name)` — restricts the row to the named columns.
    ///
    /// The qualification is read and then DROPPED: the payload is the column
    /// half. That is not a decision, it is the shape this variant already had,
    /// and it is the one place in the source algebra where a qualified
    /// reference still loses its binding.
    Select(Vec<SmolStr>),
    /// `join(User, on = Purchase.user_id == User.id)` — an inner join whose
    /// condition is a PREDICATE (ruling 17 of `SURFACE-PLAN.md`).
    ///
    /// It was `on = .k`, `USING (k)` semantics with the key named once. Two
    /// things killed that and both are already decided elsewhere: `.k` needed a
    /// `FieldRef`, and there is no `FieldRef`; and the body writes
    /// `Purchase.amount` next to `User.email`, so the qualification that
    /// `USING` existed to avoid is the thing the language now has.
    ///
    /// `alias` is the `Node as Other` of a self-join — the second name for the
    /// same source, which is the only thing that can tell the two sides apart
    /// once both are the same binding.
    Join {
        right: SmolStr,
        alias: Option<SmolStr>,
        on: HirExpr,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirProperty {
    pub key: PropertyKey,
    pub value: HirExpr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PropertyKey {
    /// `@subject = <expr>` — the mapping's identity.
    ///
    /// Not a predicate and not a property: a shape document declares the
    /// predicates of a node, and in RDF the subject IS the node, so there is no
    /// slot to assign it to. It is a slot the LANGUAGE owns, which is what the
    /// `@` marks — the same criterion Django uses («model metadata is anything
    /// that's not a field») with JSON-LD's solution, which reserves `@`+ALPHA
    /// because a user's keys are arbitrary and you cannot appropriate `id`.
    ///
    /// It replaces `PropertyKey::Iri`, and with it went the `iri` keyword and
    /// the `@subject(iri = …)` call shape that `a0d9bfa` built and that lived
    /// two hours: an identity is ASSIGNED, and a call shape would have made the
    /// language's own slot look like a function it does not have.
    Subject,
    /// `name = User.name` — a bare name.
    ///
    /// **The IRI is not here, and that is the change.** The key used to carry
    /// the fully-resolved predicate IRI, expanded from a CURIE against the
    /// file's prefix table; a bare name has no prefix to expand and the
    /// document is the only thing that knows the IRI. The name is the LAST
    /// SEGMENT of a predicate IRI the shape declares, and
    /// resolving it back is [`crate::check`]'s job, because that is where the
    /// shape is already resolved. Two predicates whose last segments coincide
    /// are an error naming both IRIs — never a numeric suffix, never a silent
    /// pick.
    Name(SmolStr),
}

/// Every BINARY operator the language has — comparison, connective, arithmetic.
///
/// It lived in `fossil-mir` until F2 §2, where the HIR needed it: an operator
/// the parser reads and the checker types cannot be defined downstream of both.
/// `fossil-mir` and the backends name this one.
///
/// **The name is now wrong and is left alone deliberately.** It has held `And`
/// and `Or` — which compare nothing — since it was written, and L5/L6 make that
/// worse rather than different: `CmpOp::Mul` is a lie in the type's name. The
/// rename to `BinOp` is a mechanical one-token change across six files, two of
/// which (`check.rs`, `infer.rs`) were under concurrent edit when this landed,
/// and a rename is the one change that cannot be merged with a conflict. It is
/// owed, and it is the whole of what is owed.
///
/// # The five arithmetic operators, and where their meaning is fixed
///
/// [`crate::check`]'s `synth_binop` decides the RESULT TYPE (`Integer` unless
/// an operand is `Float`, except for [`Self::Div`], which is always `Float`),
/// and `fossil_df::render` decides what the engine computes. Neither can be
/// read off this enum, which is a tag and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// L5 `+`. Numeric only — string concatenation is `str.concat`, and one
    /// idea does not get two spellings.
    Add,
    /// L5 `-`.
    Sub,
    /// L6 `*`.
    Mul,
    /// L6 `/` — ALWAYS float division. See `synth_binop`.
    Div,
    /// L6 `%`.
    Rem,
}

/// A `-` or a `not` — the two operators of L7 (grammar.bnf, UnaryExpr).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum UnOp {
    /// `-Row.drift` — arithmetic negation. Numeric operand, numeric result.
    Neg,
    /// `not Row.faulty` — boolean negation. Bool operand, Bool result.
    Not,
}

/// An `f64` by its IEEE-754 bits, so a float literal can live in a type that is
/// `Eq + Hash` for Salsa interning.
///
/// That requirement is why [`HirExpr`] carried integers and refused floats: the
/// old doc-comment on `IntLit` said a float «needs a representation decision the
/// type system has not made». This is the decision, and it is the smallest one
/// available — the bits are the literal, exactly, with no re-parse downstream
/// and no precision lost between the lexer and the Parquet column.
///
/// Equality is BITWISE and therefore total, which is what `Eq` demands and what
/// `f64`'s own `PartialEq` cannot give (`NaN != NaN`, `-0.0 == 0.0`). Two source
/// literals that are bitwise equal are the same literal; `-0.0` and `0.0` are
/// not, and that distinction is load-bearing — see [`UnOp::Neg`] and the
/// measurement in `negation`'s docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct FloatBits(u64);

impl FloatBits {
    /// Carry `v`.
    #[must_use]
    pub const fn new(v: f64) -> Self {
        Self(v.to_bits())
    }

    /// The value back.
    #[must_use]
    pub const fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

/// One piece of an [`HirExpr::Interpolation`]: literal text, or a hole.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum InterpolationPart {
    /// A literal run, with `{{` already resolved to `{`.
    Text(SmolStr),
    /// A hole. It is an expression like any other, and is typed like one.
    Hole(HirExpr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum HirExpr {
    /// An interpolated string — its literal runs and its holes, in order.
    ///
    /// One surface spelling reaches here, `"…{u.id}…"`. The backtick
    /// `` `…${.id}…` `` is retired and is not a token, so nothing builds an
    /// interpolation from one. A hole holds an ordinary [`HirExpr`], parsed by
    /// the parser, so nothing downstream has to know a string is made of text.
    ///
    /// It replaced a raw `Template(SmolStr)` that carried the token verbatim
    /// and was scanned again at MIR-lowering time. That is why two walkers used
    /// to be blind to the columns a subject IRI reads: there was no node to
    /// walk. Now there is.
    Interpolation(Vec<InterpolationPart>),
    /// A bare `id` → field name `"id"`: a column of the one row in scope.
    ///
    /// The anonymous row, and it is ending: every reference becomes qualified,
    /// and this variant goes with the last fixture that spells one. The
    /// leading-dot spelling `.id` that used to build it is already gone — the
    /// parser refuses a leading `.` — and what still reaches here is a bare
    /// identifier. See [`Self::ColumnRef`].
    FieldRef(SmolStr),
    /// `orders.user_id` → the `user_id` column of the row `orders` names.
    ///
    /// The qualified reference, and the only one: a column is named by the row
    /// it belongs to. It shares its CST
    /// shape with a call's callee — `io.csv` is the same `IDENT DOT IDENT` —
    /// so what separates them is the parenthesis, and what separates it from
    /// `str.slug` (a stdlib function named but not applied) is whether the
    /// head is a catalogued namespace.
    ///
    /// Both spellings are accepted while the surface is replaced in stages.
    /// That is two spellings for one idea, which this house does not keep: the
    /// sequence ends by deleting `FieldRef`, and until it does the language in
    /// this tree is mid-move, not finished.
    ColumnRef { binding: SmolStr, column: SmolStr },
    /// `"hello"` → literal text without surrounding quotes.
    StringLit(SmolStr),
    /// A full IRI, already resolved — and UNREACHABLE from the surface: the
    /// `ex:foo` CURIE that used to build one is gone, and nothing in this file
    /// constructs the variant. Deleting it reaches four other modules, so it is
    /// its own change.
    PrefixedName { iri: SmolStr },
    /// `str.slug(User.name)` — a stdlib function applied to positional arguments.
    ///
    /// `func` is the fully-qualified dotted name exactly as
    /// [`crate::stdlib`] catalogues it (`"str.slug"`), NOT a backend spelling:
    /// which `DuckDB` builtin or `DataFusion` UDF it becomes is the
    /// materializer's business, resolved from the catalog entry. Arguments are
    /// positional and recursive — `str.concat(str.trim(User.a), "-")` nests.
    Call { func: SmolStr, args: Vec<HirExpr> },
    /// `buyer = Person(User.email)` — an EDGE: «the `Person` whose identity is
    /// built from this email».
    ///
    /// `target` is the LOCAL type name as written — one of the names a
    /// `type { … } := …` binding introduced — and not the shape IRI, so a
    /// diagnostic can quote what the author wrote. `def_map`'s
    /// [`lookup_type`](crate::def_map::DefMap::lookup_type) resolves it wherever
    /// the IRI is wanted.
    ///
    /// # It is a call, and it needed no production
    ///
    /// A type name is a `PrimaryExpr` (grammar.bnf, PrimaryExpr) and application
    /// is the ordinary call (grammar.bnf, PostfixOp); what makes this a distinct
    /// HIR form is the LOWERING, which knows the type and uses THE template of
    /// that type — unique because a type has exactly ONE identity, declared once
    /// by the program as its `@subject`. That is what retired
    /// `subject_skeletons`, where an
    /// edge was GUESSED by comparing IRI-template skeletons between mappings
    /// with a `\u{1}` marker standing in for every per-row hole, so
    /// `str.slug(User.name)` counted as a constant and could match where it must
    /// not.
    ///
    /// # Binding is POSITIONAL, and the argument is the hole's finished value
    ///
    /// The Nth argument fills the Nth hole of the target's `@subject` template,
    /// in order of appearance, and it is the value that GOES IN the hole rather
    /// than an input to whatever expression the template writes there. Two
    /// consequences, both intended:
    ///
    /// - The language has ONE binding rule. `type { A, B } := …` already binds
    ///   the Nth name to the Nth declared shape, and
    ///   a second, by-name rule here would be a second idea.
    /// - The hole's spelling stays private. Binding by name would make the
    ///   identifier inside `{…}` — `{User.email}` — part of the type's public
    ///   API, so renaming a column in the mapping that DEFINES an identity would
    ///   break every mapping that references it.
    ///
    /// Arity is checked against the template's hole count
    /// ([`crate::identity::subject_templates`]) and a mismatch is a diagnostic
    /// naming both numbers, in the shape of
    /// [`ShapeBindError::Arity`](crate::def_map::ShapeBindError::Arity) — the
    /// check positional binding gives away free.
    Edge { target: SmolStr, args: Vec<HirExpr> },
    /// `18` — an integer literal.
    IntLit(i64),
    /// `0.5` — a float literal, carried as its bits. See [`FloatBits`].
    FloatLit(FloatBits),
    /// `true` / `false` — a boolean literal.
    ///
    /// It needed a `BOOL` token before it could exist. As an `IDENT`, `true` was
    /// a [`Self::FieldRef`] and `verified = true` reported `unknown column
    /// `true`` — a literal sent to look for a binding it can never have.
    BoolLit(bool),
    /// `.age >= 18`, `gross - discount` — a comparison, a boolean connective or
    /// arithmetic. One node for all three: the CST builds one `BINARY_EXPR` and
    /// the operator is the only thing that differs.
    BinOp {
        op: CmpOp,
        lhs: Box<HirExpr>,
        rhs: Box<HirExpr>,
    },
    /// `-Row.drift`, `not Row.degraded` — L7 (grammar.bnf, UnaryExpr).
    ///
    /// # Why this is a node and not a desugaring
    ///
    /// `not x` ≡ `x == false` and `-x` ≡ `0 - x` are both expressible with what
    /// [`Self::BinOp`] already carries, so the variant had to earn itself. It
    /// did, three times, and the first one is a MEASURED difference in the data:
    ///
    /// 1. **`0 - x` is not `-x` at zero.** IEEE-754 says `0.0 - 0.0` is `+0.0`
    ///    while `-(0.0)` is `-0.0`: same value under `==`, different bits
    ///    (`0x0000…` vs `0x8000…`), and different again under division
    ///    (`+inf` vs `-inf`). A `xsd:float` column written from the desugaring
    ///    loses the sign of every zero, and the corpus is a file — the bits are
    ///    what a reader gets. `-NaN` and `0 - NaN` differ in the sign bit too.
    /// 2. **A desugaring makes the IDE report an operator nobody wrote.**
    ///    `ProvenanceKind::BinaryOp` carries the operator's source spelling, and
    ///    hover would say `==` over a `not`.
    /// 3. **So would a type error.** «the left side of `==` must be Bool» names
    ///    a `==` that is not in the file.
    ///
    /// `not x` ≡ `x == false` DOES hold at every input including NULL (SQL's
    /// three-valued logic sends both to NULL), so (1) is about `-` alone. It is
    /// still one variant for both operators: two operators of one level, one
    /// node, and the alternative is a node for `-` and a desugaring for `not`,
    /// which is two answers to one question.
    UnaryOp { op: UnOp, operand: Box<HirExpr> },
    /// `.age >= 18 ? "adult" : "minor"` — the conditional.
    ///
    /// Both branches must have the same type and there is no implicit coercion
    /// (`type-system.md` §4.8), which is what makes it a total function of the
    /// row rather than a source of nullable columns.
    Ternary {
        cond: Box<HirExpr>,
        then: Box<HirExpr>,
        otherwise: Box<HirExpr>,
    },
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn lower_to_hir<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> HirFile<'db> {
    let cst = fossil_syntax::parse(db, file);
    // The `DefMap` is threaded in for the SHAPE, and that is the whole of the
    // change: a header names one of the bare names a `type { … } := …` binding
    // introduced, and the binding is what knows the IRI. It used to be threaded
    // in for the prefix table, which every function below carried a slice of.
    let dm = def_map(db, file);
    let type_names = bound_type_names(db, dm);

    let mut mappings = Vec::new();
    let mut source_pipes = Vec::new();
    for child in cst.root(db).syntax().children() {
        match child.kind() {
            fossil_syntax::SyntaxKind::MAPPING => {
                if let Some(mut m) = lower_mapping_node(db, &child, dm) {
                    // `Orders : Order from Purchase.join(User, on = …)`.
                    // `MappingHeader := IDENT SHAPE_SEP ShapeExpr 'from'
                    // Expression` (grammar.bnf), so a `from` clause derives a
                    // relation INLINE exactly as a `SourceDef` does — and until
                    // now the derivation was dropped on the floor:
                    // `source_binding_of` kept the first IDENT (`Purchase`) and
                    // the `join` was never lowered at all, so `User.email` in
                    // the body had no row to resolve against and MIR emitted
                    // one `Op::Source` where a join belonged.
                    //
                    // The relation is anonymous, and there is already a name
                    // free for it: the mapping's own. So the pipe
                    // is registered under the mapping's name and the mapping
                    // draws `from` it, which puts it through the same
                    // `source_pipes` lookup every named pipeline uses — one
                    // path, in the checker and in `fossil-mir` both.
                    if let Some(pipe) = lower_from_pipe(db, &child, &m.name, &type_names) {
                        m.source_binding = pipe.name.clone();
                        source_pipes.push(pipe);
                    }
                    mappings.push(m);
                }
            }
            fossil_syntax::SyntaxKind::SOURCE_DEF => {
                check_provider(db, &child, fossil_base::Capability::ReadRows);
                check_schema_arg(db, &child);
                if let Some(p) = lower_source_pipe(db, &child, &type_names) {
                    source_pipes.push(p);
                }
            }
            fossil_syntax::SyntaxKind::MULTI_SOURCE_DEF => {
                check_provider(db, &child, fossil_base::Capability::ReadRows);
                check_schema_arg(db, &child);
            }
            fossil_syntax::SyntaxKind::TYPE_DEF => {
                check_provider(db, &child, fossil_base::Capability::ReadTypes);
                check_renames(db, file, &child);
            }
            _ => {}
        }
    }
    HirFile::new(db, mappings, source_pipes)
}

/// **Every `@rename` above a `type` binding, checked against the binding and
/// against the document** — with a real span, because this is the one place
/// that has the nodes.
///
/// `crate::def_map::def_map` reads the same attributes and cannot report on
/// them: it is signatures-only and runs where `delay_span_bug` is not valid, so
/// it silently keeps the renames that match a bound name and drops the rest.
/// «Drops the rest» is the failure this function exists to stop — a `@rename`
/// that renames nothing is a line the author wrote to fix a collision, which
/// then still collides, and the diagnostic they get is about the collision they
/// thought they had repaired.
///
/// Two ways to write one nobody can act on, and each gets its own message:
///
/// 1. **The name is not one this binding introduces** — `@rename(Persn, …)`
///    above `type { Person }`. A did-you-mean over the names it does introduce.
/// 2. **The predicate is not one the shape declares.** This is the valuable one
///    and it is why the check reads the document: the repair is written by
///    copying an IRI out of a diagnostic, and an IRI copied wrong is invisible —
///    the rename simply never fires and the short name stays what it was.
///
/// A binding whose document could not be read is NOT reported here. It has
/// already been reported, as a `ShapeBindError`, against the binding itself; a
/// second message per rename would be N messages about one missing file.
fn check_renames(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    type_def: &fossil_syntax::SyntaxNode,
) {
    use fossil_syntax::SyntaxKind;

    let renames = crate::def_map::parse_renames(type_def);
    if renames.is_empty() {
        return;
    }
    let dm = def_map(db, file);
    // The names THIS binding introduces, in order — not the file's, because a
    // `@rename` is scoped to the binding it sits on (grammar.bnf, TypeDef).
    let members: Vec<SmolStr> = {
        let toks: Vec<_> = type_def
            .descendants_with_tokens()
            .filter_map(fossil_syntax::SyntaxElement::into_token)
            .collect();
        let open = toks.iter().position(|t| t.kind() == SyntaxKind::LBRACE);
        let close = toks.iter().position(|t| t.kind() == SyntaxKind::RBRACE);
        match (open, close) {
            (Some(o), Some(c)) if c > o => toks[o + 1..c]
                .iter()
                .filter(|t| t.kind() == SyntaxKind::IDENT)
                .map(|t| SmolStr::from(t.text()))
                .collect(),
            _ => Vec::new(),
        }
    };

    // One `RENAME` node per entry, in the same order `parse_renames` walked
    // them, so each diagnostic underlines its own line rather than the whole
    // attribute.
    let rename_nodes: Vec<fossil_syntax::SyntaxNode> = type_def
        .children()
        .filter(|c| c.kind() == SyntaxKind::RENAME_ATTR)
        .flat_map(|a| a.children())
        .filter(|c| c.kind() == SyntaxKind::RENAME)
        .collect();

    for (i, r) in renames.iter().enumerate() {
        let node = rename_nodes.get(i).unwrap_or(type_def);
        if !members.contains(&r.type_name) {
            let suggestion =
                crate::didyoumean::did_you_mean(&r.type_name, members.iter().map(SmolStr::as_str));
            let tail = suggestion.map_or_else(
                || {
                    if members.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " — this binding introduces {}",
                            members
                                .iter()
                                .map(|m| format!("`{m}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                },
                |s| format!(" — did you mean `{s}`?"),
            );
            diagnose(
                db,
                node,
                format!(
                    "`@rename` names `{}`, which this `type` binding does not introduce{tail}. A \
                     rename renames a predicate OF one bound type.",
                    r.type_name
                ),
            );
            continue;
        }
        // The document, through the bound name. A name that bound no shape has
        // already said why.
        let Some(shape_iri) = dm.lookup_type(db, r.type_name.as_str()) else {
            continue;
        };
        // The PAIR, not the path. The constructor is what selects the row that
        // reads the document (ruling 13), and `output_shape_binding` hands both
        // over together precisely so no caller can take one and forget the
        // other — which is how the two came apart in the first place.
        let Some((constructor, document)) = dm.output_shape_binding(db) else {
            continue;
        };
        let Ok(shapes) =
            crate::shapes::decoded_document(db, file, constructor.as_deref(), document.as_str())
        else {
            continue;
        };
        let Some(shape) = shapes.lookup(shape_iri.as_str()) else {
            continue;
        };
        if shape
            .properties
            .iter()
            .any(|c| c.predicate == r.predicate.as_str())
        {
            continue;
        }
        let declared: Vec<&str> = shape
            .properties
            .iter()
            .map(|c| c.predicate.as_str())
            .collect();
        let suggestion = crate::didyoumean::did_you_mean(&r.predicate, declared.iter().copied());
        let tail = suggestion.map_or_else(
            || {
                if declared.is_empty() {
                    " — it declares none".to_string()
                } else {
                    format!(
                        " — it declares {}",
                        declared
                            .iter()
                            .map(|d| format!("`{d}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            },
            |s| format!(" — did you mean `{s}`?"),
        );
        diagnose(
            db,
            node,
            format!(
                "`{}` declares no predicate `{}`, so this rename never fires{tail}. The \
                 predicate is a FULL IRI, because two predicates that collide differ only \
                 before their last segment.",
                r.type_name, r.predicate
            ),
        );
    }
}

/// **The provider a binding names, checked against what its POSITION asks of
/// it** — ruling 13 of `SURFACE-PLAN.md`, with a real span.
///
/// The row declares its capabilities; where the binding is written decides which
/// one is asked for. `User := io.csv(…)` asks for rows; `type { P } := io.shex(…)`
/// asks for types. Asking a row for a capability it does not declare is an error
/// that **names both**, and it is worded by the ROW
/// ([`fossil_base::Provider::decline_capability`]) rather than here: what a
/// language does and does not carry is the row's to explain, and the core only
/// carries the sentence.
///
/// This is reported here and not in [`crate::shapes::resolve_target_shape`] for
/// two reasons. The span: `node.text_range()` covers the binding the author
/// wrote, and the per-mapping path has only the mapping header. And the trap
/// this repo has already paid for once — `lower_property` ends in `return None`
/// and `body.rs` skips it without a word, so properties disappear in silence. A
/// provider mismatch that produced no diagnostic would be that failure in a new
/// place.
///
/// # The extension is checked in TYPE position only
///
/// A shape document's extension is what ruling 13 names
/// (`io.shex("catalogue.ttl")`), and [`crate::shapes::decoded_document`] enforces
/// it for every document however it was named. A DATA URI is not checked, and
/// deliberately: nothing has ever required one to have an extension,
/// `s3://bucket/export` and `@warehouse/daily` are both legal today, and turning
/// the row's `extensions` list into a gate on the data side would reject working
/// programs for a rule no decision states. There the list stays what it has
/// always been — the `fossil providers` listing.
fn check_provider(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    wanted: fossil_base::Capability,
) {
    let (Some(constructor), uri) = crate::def_map::parse_source_call(node) else {
        // No call-shaped right-hand side. The parser has already refused
        // whatever was there, or it is a derived binding
        // (`Adults := User.where(…)`) that names no provider at all.
        return;
    };
    let table = db.system().providers();
    if !answers_about(table, wanted) {
        return;
    }
    let Some(row) = fossil_base::provider(table, &constructor) else {
        // Only a name that LOOKS like a provider is reported here. A derived
        // binding reads as `User.where` and belongs to `fossil-mir`'s
        // `resolve_source`, which already has a message for it.
        if constructor.starts_with("io.") {
            diagnose_item(
                db,
                node,
                format!(
                    "`{constructor}` is not a provider this host installs — it has {}",
                    table
                        .iter()
                        .map(|p| format!("`{}`", p.constructor()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        return;
    };
    if !row.provides(wanted) {
        diagnose_item(db, node, row.decline_capability(wanted, table));
        return;
    }
    if wanted == fossil_base::Capability::ReadTypes
        && let Some(uri) = uri
        && !row.accepts(&uri)
    {
        diagnose_item(db, node, row.decline_extension(&uri));
    }
}

/// **Does this host answer questions about `wanted` at all?**
///
/// A host installs the rows it can serve, and a host with NO row for a
/// capability is not in that business — `fossil-df-wasm`'s executor installs no
/// type reader on purpose, because its shape arrives already decoded and a
/// second answer inside the database would be a second truth (`ExecutorSystem`
/// says so in its own docs). That silence is correct rather than
/// degraded: an undeclared predicate is legal in an open world, so a program
/// compiled with no output contract type-checks with no expected types.
///
/// Without this the checks below would tell every such host that `io.shex` "is
/// not a provider", which is false twice over: it is a provider of the LANGUAGE,
/// and the host not having installed a row for it is the host's answer, not the
/// program's mistake. On a host that installs one — the engine, the LSP, the
/// editor — the same name really is unknown and the message stands.
fn answers_about(table: &[&'static fossil_base::Provider], wanted: fossil_base::Capability) -> bool {
    table.iter().any(|p| p.provides(wanted))
}

/// **The `schema =` argument, which names a provider like every other position
/// that names a document.**
///
/// ```text
/// { Person, Org } := io.rdf("g.ttl", schema = io.shex("x.shex"))
/// { Product }     := io.rdf("g.ttl", schema = io.shacl("s.ttl"))
/// ```
///
/// It used to be a bare path, and it was the last place in the language where a
/// document arrived without a row to read it — so the decoder came from the
/// document's own EXTENSION, the one dispatch that did not go by name. With this
/// there is a single rule: **where there is a document, there is a row that
/// names it.**
///
/// The alternative was killing `schema =` and taking every document from a
/// `type { … }` binding. They are not the same document: `schema =` shapes the
/// INPUT rows and `type { … }` is the OUTPUT contract, and nothing says a
/// program's two ends read one file.
///
/// The span is the ARGUMENT's, not the binding's — `crate::def_map::SchemaArg`
/// carries it out of the one scanner that reads this argument, because a second
/// token scan over here is how the two spellings drifted apart the first time.
fn check_schema_arg(db: &dyn fossil_base::Db, node: &fossil_syntax::SyntaxNode) {
    let Some(arg) = crate::def_map::parse_schema_arg(node) else {
        return;
    };
    let table = db.system().providers();
    if !answers_about(table, fossil_base::Capability::ReadTypes) {
        return;
    }
    let Some(constructor) = arg.provider else {
        emit_item(
            db,
            arg.span,
            "`schema =` names a document, and a document is named by the provider \
             that reads it: write `schema = io.shex(\"…\")` or \
             `schema = io.shacl(\"…\")`, never a bare path — the row that reads a \
             document is the one the program names, not the one its extension \
             happens to match"
                .to_string(),
        );
        return;
    };
    let Some(row) = fossil_base::provider(table, &constructor) else {
        emit_item(
            db,
            arg.span,
            format!(
                "`{constructor}` is not a provider this host installs — it has {}",
                table
                    .iter()
                    .map(|p| format!("`{}`", p.constructor()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        return;
    };
    if !row.provides(fossil_base::Capability::ReadTypes) {
        emit_item(
            db,
            arg.span,
            row.decline_capability(fossil_base::Capability::ReadTypes, table),
        );
        return;
    }
    if let Some(document) = arg.document
        && !row.accepts(&document)
    {
        emit_item(db, arg.span, row.decline_extension(&document));
    }
}

/// Lower a `SOURCE_DEF` whose right-hand side derives a relation from another
/// binding — `Adults := User.where(User.age >= 18)`. Returns `None` for the
/// ordinary `User := io.csv("u.csv")` shape, which carries no expressions and
/// belongs to [`crate::def_map`].
///
/// # The spine is a `POSTFIX_EXPR` chain, and it nests to the LEFT
///
/// This walked a `PIPELINE_EXPR` spine until ruling 7 of 2026-08-11 retired
/// `|>`; the member call is the spelling now, and it builds a different tree for
/// the same shape. `User.where(p).select(c)` is
///
/// ```text
/// POSTFIX_EXPR(call)                       ← .select(c)
///   POSTFIX_EXPR(member `select`)
///     POSTFIX_EXPR(call)                   ← .where(p)
///       POSTFIX_EXPR(member `where`)
///         LITERAL_EXPR(User)               ← the head
///       ARG_LIST(p)
///   ARG_LIST(c)
/// ```
///
/// so the walk goes down the callee of each call, collecting a verb per member
/// node, and the stages come back out in written order after one reverse.
///
/// # What separates this from `io.csv("u.csv")`, which builds the SAME tree
///
/// Nothing structural: `io.csv(…)` is also a call whose callee is a member
/// access, and its head is also a bare name. What separates them is that `io` is
/// a CATALOGUED head and `User` is not — a question about the catalogue, asked
/// of the catalogue ([`FunctionRegistry::is_catalogued_head`]). This is the
/// receiver doing the work the string-matching used to do badly: before it, the
/// two were told apart by which SyntaxKind the parser happened to build.
fn lower_source_pipe(
    db: &dyn fossil_base::Db,
    source_def: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirSourcePipe> {
    use fossil_syntax::SyntaxKind;

    let name = source_def
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))?;

    let rhs = source_def
        .children()
        .find(|c| c.kind() == SyntaxKind::EXPR)?
        .children()
        .next()?;

    let range = source_def.text_range();
    lower_pipe_expr(db, &rhs, name, (range.start().into(), range.end().into()), types)
}

/// The `from` clause of a mapping header, when it derives a relation rather than
/// naming one — `Orders : Order from Purchase.join(User, on = …)`.
///
/// The same production as a `SourceDef`'s right-hand side (`MappingHeader :=
/// IDENT SHAPE_SEP ShapeExpr 'from' Expression`), so it is the same walk, under
/// the one name that is free for a relation nobody named: the mapping's own.
///
/// `None` for the ordinary `from Adults`, which names a relation and derives
/// nothing.
fn lower_from_pipe(
    db: &dyn fossil_base::Db,
    mapping: &fossil_syntax::SyntaxNode,
    mapping_name: &SmolStr,
    types: &[SmolStr],
) -> Option<HirSourcePipe> {
    use fossil_syntax::SyntaxKind;

    let header = mapping
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)?;
    let rhs = header
        .children()
        .find(|c| c.kind() == SyntaxKind::EXPR)?
        .children()
        .next()?;
    let range = header.text_range();
    lower_pipe_expr(
        db,
        &rhs,
        mapping_name.clone(),
        (range.start().into(), range.end().into()),
        types,
    )
}

/// The pipeline an expression spells, under a name and a span its caller owns.
///
/// One walk for the two positions a relation can be derived in — a `SourceDef`'s
/// right-hand side and a mapping header's `from` — because `grammar.bnf` gives
/// them one production and two walks would be two answers to one question.
fn lower_pipe_expr(
    db: &dyn fossil_base::Db,
    rhs: &fossil_syntax::SyntaxNode,
    name: SmolStr,
    span: (u32, u32),
    types: &[SmolStr],
) -> Option<HirSourcePipe> {
    let rhs = rhs.clone();
    // Walk down the callee spine, collecting (verb, call-node) pairs.
    let mut stages: Vec<(SmolStr, fossil_syntax::SyntaxNode)> = Vec::new();
    let mut head = rhs;
    loop {
        if !is_call(&head) {
            break;
        }
        let Some(callee) = head.children().next() else {
            break;
        };
        let Some(verb) = member_name(&callee) else {
            break;
        };
        let Some(receiver) = callee.children().next() else {
            break;
        };
        stages.push((verb, head.clone()));
        head = receiver;
    }
    stages.reverse();

    if stages.is_empty() {
        return None;
    }

    let base = bare_name(&head)?;

    // `io.csv("u.csv")` reaches here as one stage `csv` over the head `io`.
    // A catalogued head is a namespace call, not a relation derived from a
    // binding, and `def_map` owns it.
    if crate::stdlib::stdlib().is_catalogued_head(base.as_str()) {
        return None;
    }

    let mut ops = Vec::with_capacity(stages.len());
    for (verb, call) in &stages {
        match lower_source_stage(db, verb, call, &name, types) {
            Some(op) => ops.push(op),
            // **A stage that did not lower must not delete the binding.**
            // `Working := Row.where(not Row.faulty)` — `not` has no `HirExpr`
            // yet, so the `where` came back `None` and the whole pipeline
            // vanished with it; `Working` then resolved as a binding that reads
            // nothing, and every `Row.column` in the mapping that draws from it
            // was told `Row` is a row this mapping does not have. That is a
            // cascade of FALSE messages on top of the one true one, which the
            // stage has already emitted.
            //
            // Only `where` is dropped, and only because it is the one verb that
            // changes neither the columns nor the names: the relation minus a
            // filter has exactly the type the relation has. `select` and `join`
            // rewrite the row, so a pipeline missing one of them would resolve
            // references against a row the program does not describe — the
            // wrong answer, which is worse than none.
            None if verb == "where" => {}
            None => return None,
        }
    }

    Some(HirSourcePipe {
        name,
        base,
        ops,
        span,
    })
}

/// Does this `POSTFIX_EXPR` carry a `(`? That is the whole of what separates a
/// call from a member access — the CST shape is otherwise identical, which is
/// the same fact `lower_postfix` turns on.
fn is_call(node: &fossil_syntax::SyntaxNode) -> bool {
    use fossil_syntax::SyntaxKind;
    node.kind() == SyntaxKind::POSTFIX_EXPR
        && node
            .children_with_tokens()
            .filter_map(fossil_syntax::SyntaxElement::into_token)
            .any(|t| t.kind() == SyntaxKind::LPAREN)
}

/// The member a `POSTFIX_EXPR` member-access names — `where` in `User.where`.
/// `None` for a call node and for anything that is not member access.
fn member_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    if node.kind() != SyntaxKind::POSTFIX_EXPR || is_call(node) {
        return None;
    }
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| t.kind() == SyntaxKind::DOT || t.kind() == SyntaxKind::IDENT)
        .collect();
    match toks.as_slice() {
        [d, i] if d.kind() == SyntaxKind::DOT && i.kind() == SyntaxKind::IDENT => {
            Some(SmolStr::from(i.text()))
        }
        _ => None,
    }
}

/// One stage of a source pipeline — `where(...)`, `select(...)` or `join(...)`.
///
/// `verb` is the member the call names; `stage` is the call node, which is where
/// the `ARG_LIST` hangs.
fn lower_source_stage(
    db: &dyn fossil_base::Db,
    verb: &SmolStr,
    stage: &fossil_syntax::SyntaxNode,
    pipe: &str,
    types: &[SmolStr],
) -> Option<HirSourceOp> {
    use fossil_syntax::SyntaxKind;

    // Positional arguments, the `on = ...` named one and the `X as Y` alias,
    // kept apart: the verbs read them differently and a positional `on` is not
    // the same word.
    let args: Vec<fossil_syntax::SyntaxNode> = stage
        .children()
        .find(|c| c.kind() == SyntaxKind::ARG_LIST)
        .into_iter()
        .flat_map(|l| l.children())
        .collect();
    let positional: Vec<fossil_syntax::SyntaxNode> = args
        .iter()
        .filter(|a| a.kind() == SyntaxKind::ARG)
        .filter_map(|a| a.children().next())
        .collect();
    let named = |want: &str| -> Option<fossil_syntax::SyntaxNode> {
        args.iter()
            .filter(|a| a.kind() == SyntaxKind::NAMED_ARG)
            .find(|a| {
                a.children_with_tokens()
                    .filter_map(fossil_syntax::SyntaxElement::into_token)
                    .any(|t| t.kind() == SyntaxKind::IDENT && t.text() == want)
            })
            .and_then(|a| a.children().next())
    };

    match verb.as_str() {
        "where" => {
            if positional.len() != 1 {
                diagnose(
                    db,
                    stage,
                    format!(
                        "`where` takes one predicate, and `{pipe}` gives it {}. \
                         e.g. `where(User.age >= 18)`.",
                        positional.len()
                    ),
                );
                return None;
            }
            Some(HirSourceOp::Where(lower_expr_inner(db, &positional[0], types)?))
        }
        "select" => {
            if positional.is_empty() {
                diagnose(
                    db,
                    stage,
                    format!(
                        "`select` in `{pipe}` names no column. \
                         e.g. `select(User.id, User.name)`."
                    ),
                );
                return None;
            }
            let mut cols = Vec::with_capacity(positional.len());
            for arg in &positional {
                let Some(HirExpr::ColumnRef { column, .. }) = lower_expr_inner(db, arg, types)
                else {
                    diagnose(
                        db,
                        arg,
                        format!(
                            "`select` in `{pipe}` takes qualified column references and this is \
                             not one. e.g. `select(User.id, User.name)`."
                        ),
                    );
                    return None;
                };
                cols.push(column);
            }
            Some(HirSourceOp::Select(cols))
        }
        "join" => {
            // `Purchase.join(User, on = …)` and the self-join
            // `Node.join(Node as Other, on = …)`. The alias is an `ALIAS_ARG`
            // and not a positional argument, so the two are read apart.
            let (right, alias) = match alias_arg(&args) {
                Some((source, alias)) => (source, Some(alias)),
                None => {
                    let Some(right) = positional.first().and_then(bare_name) else {
                        diagnose(
                            db,
                            stage,
                            format!(
                                "`join` in `{pipe}` does not name the source binding it joins. \
                                 e.g. `join(User, on = Purchase.user_id == User.id)`."
                            ),
                        );
                        return None;
                    };
                    (right, None)
                }
            };
            // `on = <predicate>`, and the predicate form is the one that
            // survived: `on = .k` needed a `FieldRef` to name a column of an
            // anonymous row, and there is no `FieldRef` — a leading `.` starts
            // nothing — so the `USING (k)` semantics, where the key is named
            // once and both sides are assumed to spell it the same, has nothing
            // left to write itself with. `on = Purchase.user_id == User.id` says which
            // row each side belongs to, which is what qualification is for.
            let Some(on) = named("on").and_then(|n| lower_expr_inner(db, &n, types)) else {
                diagnose(
                    db,
                    stage,
                    format!(
                        "`join` in `{pipe}` needs `on = <predicate>`, a condition relating the \
                         two sides by qualified column. \
                         e.g. `join(User, on = Purchase.user_id == User.id)`."
                    ),
                );
                return None;
            };
            Some(HirSourceOp::Join { right, alias, on })
        }
        other => {
            diagnose(
                db,
                stage,
                format!(
                    "`{other}` is not a relation verb fossil lowers. The catalogue has {}, \
                     and the lowering implements `where`, `select` and `join`.",
                    crate::stdlib::stdlib()
                        .members_of(crate::stdlib::Receiver::Relation)
                        .count()
                ),
            );
            None
        }
    }
}

/// Is the thing left of the dot a VALUE, as opposed to a NAME?
///
/// The two paths a dot can take — the dotted TYPE path and the MEMBER path over
/// a value — are told apart here, and the criterion is
/// syntactic on purpose. A bare `IDENT` to the left of a dot is a NAME — a
/// namespace (`io`), a type (`str`, `Person`) or a binding (`User`) — and the
/// dotted type path is what reads it. Anything compound (`User.email`,
/// `str.trim(x)`, `"abc"`) is a value, and the member path is what reads that.
///
/// Doing it by shape rather than by consulting the catalogue keeps the two paths
/// disjoint: were a bare name allowed to take the member path, `User.where(…)`
/// and `io.csv(…)` would both resolve as members of a value and the type path
/// would never run.
fn lower_receiver_is_value(node: &fossil_syntax::SyntaxNode) -> bool {
    bare_name(node).is_none()
}

/// The `X as Y` of a self-join, read off an `ARG_LIST`'s children.
///
/// `ALIAS_ARG := IDENT 'as' IDENT` (grammar.bnf, AliasArg). Returns the source
/// being aliased and the alias, in that order.
fn alias_arg(args: &[fossil_syntax::SyntaxNode]) -> Option<(SmolStr, SmolStr)> {
    use fossil_syntax::SyntaxKind;
    let node = args.iter().find(|a| a.kind() == SyntaxKind::ALIAS_ARG)?;
    let idents: Vec<SmolStr> = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))
        .collect();
    // Three IDENTs: the source, the contextual word `as`, and the alias.
    match idents.as_slice() {
        [source, kw, alias] if kw == "as" => Some((source.clone(), alias.clone())),
        _ => None,
    }
}

/// The text of a node that is exactly one bare `IDENT` — a binding name or a
/// verb. `io.csv` is a dotted callee and deliberately does NOT match.
fn bare_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    if node.kind() != SyntaxKind::LITERAL_EXPR {
        return None;
    }
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect();
    match toks.as_slice() {
        [t] if t.kind() == SyntaxKind::IDENT => Some(SmolStr::from(t.text())),
        _ => None,
    }
}

/// Report a node in the MAPPING-RELATIVE frame — the default one.
///
/// **Read [`diagnose_item`] before reaching for this.** The two differ by one
/// method call, both compile, and picking wrong is invisible in the fixture
/// most tests use: `crate::spans::rebase_to_file` shifts a mapping-relative
/// span by the mapping's start, which is ZERO for the first mapping in a file
/// and wrong for every other one. A `hello.fossil` with a single mapping cannot
/// tell the two apart.
///
/// This one is right only for a span measured inside a re-rooted mapping
/// subtree — what `crate::body` gets from [`lower_property_public`], where
/// rowan has reset the offsets to zero. Anything reached from [`lower_to_hir`]
/// walks the whole-file CST and its offsets are already file-absolute:
/// [`diagnose_item`].
fn diagnose(db: &dyn fossil_base::Db, node: &fossil_syntax::SyntaxNode, message: String) {
    let range = node.text_range();
    Diagnostic::new(
        Severity::Error,
        message,
        Span::new(range.start().into(), range.end().into()),
    )
    .accumulate(db);
}

/// Report a TOP-LEVEL item, whose span is file-absolute.
///
/// Everything this module emits is reached from [`lower_to_hir`], which walks
/// the WHOLE-FILE CST, so every range taken off a node here is already an
/// offset into the file. A top-level binding is not inside any mapping, and
/// `crate::spans::rebase_to_file` shifts anything left in the default
/// `SpanFrame::MappingRelative` frame by a mapping's start — which is exactly
/// what [`fossil_base::Diagnostic::file_absolute`] exists to prevent, and which
/// a one-mapping fixture cannot catch, because there the shift is zero.
///
/// The mapping-relative counterpart is `crate::body`'s: it calls
/// [`lower_property_public`] over a re-rooted mapping node whose offsets rowan
/// resets to zero. Different frame, different emitter, and not in this
/// module.
fn diagnose_item(db: &dyn fossil_base::Db, node: &fossil_syntax::SyntaxNode, message: String) {
    let range = node.text_range();
    emit_item(
        db,
        Span::new(range.start().into(), range.end().into()),
        message,
    );
}

/// [`diagnose_item`] for a span that is not a whole node — an argument inside a
/// call, whose extent the scanner that read it measured.
fn emit_item(db: &dyn fossil_base::Db, span: Span, message: String) {
    Diagnostic::new(Severity::Error, message, span)
        .file_absolute()
        .accumulate(db);
}

// `lookup_prefix` lived here — four callers, and it WAS the language: it turned
// `ex` into `https://example.org/` against the file's `prefix` lines. There are
// no `prefix` lines — a vocabulary declaration is not a form of this language —
// and no CURIE to expand: a `:` that is not a mapping header or a ternary is an
// error. So the function and the `&[PrefixEntry]` slice its
// four callers threaded between them are both gone. What resolves a name now is
// `DefMap::lookup_type`, against a document — see `lower_mapping_node`.

/// Lower a `MAPPING`'s header into its [`HirMapping`] signature.
///
/// `db` is threaded in for one reason: a header this cannot read makes the
/// WHOLE MAPPING disappear from the HIR, and it used to do that without a word.
/// Every `None` below either follows a parse error the parser already reported,
/// or emits its own.
///
/// `dm` is threaded in for the shape. `ShapeExpr := IDENT` (grammar.bnf,
/// ShapeExpr) — one of the names a `type { … } := io.shex(…)` binding
/// introduced — so the IRI comes from the binding rather than from a prefix
/// expansion the program spelled out.
fn lower_mapping_node<'db>(
    db: &'db dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    dm: crate::def_map::DefMap<'db>,
) -> Option<HirMapping> {
    use fossil_syntax::SyntaxKind;

    let header = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)?;
    // Properties are NOT collected here — the body() Salsa
    // query owns them. The MAPPING_BODY's presence is no longer required for
    // a successful header lowering; an empty-bodied mapping is still a valid
    // HirMapping signature.

    // Phase 2 plan 02-03 wraps the header's shape and source in composite
    // sub-nodes:
    //
    //   MAPPING_HEADER
    //     IDENT "Users"                     -- direct token: mapping name
    //     SHAPE_SEP ":"
    //     SHAPE_EXPR
    //       IDENT "Person"                  -- the shape NAME, bare
    //     KW_FROM "from"
    //     EXPR
    //       LITERAL_EXPR
    //         IDENT "Adults"                -- the from-source expression
    //
    // Phase 1's flat "first four IDENTs" shortcut no longer matches; walk the
    // sub-nodes by kind to extract each header field cleanly.
    let name = header
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))?;

    // ShapeExpr → its one IDENT: the shape's NAME (grammar.bnf, ShapeExpr).
    // There is no `IRI_EXPR` under it any more and no prefix to expand — the
    // name is one of those a `type { … } := io.shex(…)` binding introduced, and
    // that binding already resolved it against the document, positionally.
    //
    // `.find(…)` used to mean "the first of however many the intersection had",
    // and the rest were dropped here without a word — `A & B` checked against
    // `A` alone. The `&` left the grammar rather than the drop being made loud.
    let shape_expr = header
        .children()
        .find(|c| c.kind() == SyntaxKind::SHAPE_EXPR)?;
    let Some(shape_name) = shape_expr
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))
    else {
        // No name at all — the parser has already refused whatever was there
        // (a CURIE, a `<…>`, or nothing). Returning `None` here would delete the
        // mapping AND say nothing about it, which is the failure this whole
        // step exists to remove, so the mapping survives with no shape and
        // `shapes::resolve_target_shape` reads the empty IRI as «no shape
        // clause». The parser's diagnostic is the one that names the form.
        return Some(HirMapping {
            name,
            shape_iri: SmolStr::default(),
            source_binding: source_binding_of(&header)?,
        });
    };

    // The one resolution: a bare name against the file's type bindings.
    //
    // A name NOBODY bound is reported and the mapping still lowers, carrying an
    // empty shape IRI. That split is deliberate and it is the correction this
    // commit makes: the old code returned `None` on an unresolvable shape, so a
    // header one `prefix` line away from right produced NO HirMapping — its
    // properties never lowered, `body()` never ran on them, and every `ExprId`
    // after it shifted. A mapping that cannot name its shape is still a mapping
    // whose body the author wants checked.
    let shape_iri = match dm.lookup_type(db, shape_name.as_str()) {
        Some(iri) => iri,
        // The name is written and resolves to no shape, and there are TWO
        // reasons for that. `lookup_type` answers `None` to both, which is what
        // made the second one silent.
        None => {
            // `diagnose_item`, not `diagnose`: this is reached from
            // `lower_to_hir`, which walks the WHOLE-FILE CST, so the range is
            // already file-absolute. The default `SpanFrame::MappingRelative`
            // would make `spans::rebase_to_file` shift it by the mapping's own
            // start — invisible in a one-mapping file and wrong in every other.
            // `diagnose_item`, not `diagnose`. This is reached only from
            // `lower_to_hir`, which walks the WHOLE-FILE CST, so `shape_expr`'s
            // range is already a file offset. Left in the default
            // `MappingRelative` frame, `spans::rebase_to_file` would shift it by
            // the mapping's own start — zero for the first mapping in a file,
            // and wrong for every one after it.
            diagnose_item(db, &shape_expr, unbound_shape_message(db, dm, &shape_name));
            SmolStr::default()
        }
    };

    Some(HirMapping {
        name,
        shape_iri,
        source_binding: source_binding_of(&header)?,
    })
}

/// Why a shape name in a mapping header resolved to nothing.
///
/// Two situations, and `DefMap::lookup_type` returns `None` for both:
///
/// 1. **Nobody bound the name.** A typo, or a `type { … } = io.shex(…)` line
///    that was never written.
/// 2. **The binding is there and it FAILED.** `type { Person } =
///    io.shex("shop.shex")` with no such document, an unreadable one, one no
///    decoder claims, or more names than the document declares shapes.
///    `TypeEntry::shape_error` records which, as a
///    [`ShapeBindError`](crate::def_map::ShapeBindError).
///
/// Case 2 had no reader in the workspace: `DefMap::lookup_type_error`'s only
/// callers were its own unit tests, so the cause was computed, stored and
/// dropped. What the author got instead was case 1's message, and that message
/// then listed the declared names — INCLUDING the one it had just said was not
/// declared. So a missing shape document read as a misspelt name, and the
/// evidence contradicted itself in the same sentence.
///
/// This is the mirror of [`crate::infer`]'s treatment of a source binding that
/// bound no shape (`lookup_source_shape_error`), which has always reported by
/// cause. The two sides of the same table now behave the same way.
fn unbound_shape_message(
    db: &dyn fossil_base::Db,
    dm: crate::def_map::DefMap<'_>,
    shape_name: &SmolStr,
) -> String {
    use crate::def_map::ShapeBindError;

    if let Some(err) = dm.lookup_type_error(db, shape_name.as_str()) {
        return match err {
            ShapeBindError::NoSchema => format!(
                "`{shape_name}` is declared and bound nothing: its `type` binding names \
                 no document. Give it one — `type {{ {shape_name} }} = io.shex(\"shop.shex\")`."
            ),
            ShapeBindError::Unreadable { path, cause } => format!(
                "`{shape_name}` is declared and bound nothing: its document `{path}` \
                 could not be read ({cause}), so this mapping is checked against nothing"
            ),
            ShapeBindError::Unparseable { path, cause } => format!(
                "`{shape_name}` is declared and bound nothing: its document `{path}` \
                 could not be read as a shape document ({cause}), so this mapping is \
                 checked against nothing"
            ),
            ShapeBindError::Arity { declared, named } => format!(
                "`{shape_name}` is declared and bound nothing: the binding names {named} \
                 shape(s) and the document declares {declared}. Names bind by POSITION, \
                 so there is no {named}th shape for it to take."
            ),
        };
    }

    // Case 1: the name is not in the table at all. Only now is it honest to
    // list what the table does hold.
    let declared: Vec<&str> = dm.types(db).iter().map(|t| t.name.as_str()).collect();
    let known = if declared.is_empty() {
        "this program declares no `type { … } = io.shex(…)` binding, so it has no \
         shape names at all"
            .to_string()
    } else {
        format!("the names it binds are {}", declared.join(", "))
    };
    format!(
        "`{shape_name}` is not a shape this program declares, so this mapping is \
         checked against nothing: {known}."
    )
}

/// The name `from` draws on — the first IDENT of the header's `EXPR`.
///
/// `from Adults`, `from User.where(User.age >= 18)` and
/// `from Purchase.join(User, on = …)` are one production (grammar.bnf,
/// MappingHeader), and the first IDENT of all three is the relation being read.
fn source_binding_of(header: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    header
        .children()
        .find(|c| c.kind() == SyntaxKind::EXPR)
        .and_then(|expr_node| {
            expr_node
                .descendants_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::IDENT)
        })
        .map(|t| SmolStr::from(t.text()))
}

/// Public-to-the-crate adapter so [`crate::body::body`] can re-use the same
/// `PROPERTY` lowering logic without duplicating it. Body
/// content is owned by the `body()` Salsa query, but the per-property
/// lowering rules live here next to their natural
/// home (`HirProperty` / `HirExpr`).
///
/// `db` is threaded through so that a property this cannot read says so. That
/// claim used to be made about the property KEY and was false — the key's
/// prefix lookup was a bare `?`, so `name = .name` with no `prefix ex:` line
/// dropped the property in silence while the doc comment here said it did not.
/// Now every `None` in this function has emitted a diagnostic first.
///
/// A `prefixes: &[PrefixEntry]` slice used to come in beside `db`, for the
/// VALUE position — `${ex:}` in an interpolation was the last place a CURIE was
/// written. A hole takes an expression and nothing else, so it is not written
/// anywhere now, and the parameter is gone from this function and from the
/// eight below it.
///
/// `types` came in as it left, and it is not the same kind of parameter. The
/// prefix table was carried through nine functions to be looked up in none of
/// them; this one decides a FORM. `Person(User.email)` and `str.slug(x)` are
/// the same CST — a name applied to arguments — and the only thing that tells
/// an edge constructor from a stdlib call is whether the name is one a
/// `type { … } := …` binding introduced. The lowering is where
/// that has to be decided, because the two produce different HIR
/// ([`HirExpr::Edge`] against [`HirExpr::Call`]) and everything downstream
/// matches on it.
pub(crate) fn lower_property_public(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirProperty> {
    lower_property(db, node, types)
}

fn lower_property(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirProperty> {
    use fossil_syntax::SyntaxKind;

    // `PropertyLhs` is a bare name: the last segment of a
    // predicate IRI the shape document declares. This comment named `KW_IRI`
    // twice — the `iri =` keyword, deleted from the lexer when the identity
    // became `@subject = <expr>` — and `IRIExpr`, which grammar.bnf declares
    // absent along with `PrefixedName` and `LocalName`.
    let Some(lhs_node) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_LHS)
    else {
        diagnose(
            db,
            node,
            "this property has no name on its left, so it is not written".to_string(),
        );
        return None;
    };
    // Use `descendants_with_tokens` so nested IDENT/SHAPE_SEP tokens are still
    // discoverable. Skip trivia.
    let lhs_toks: Vec<_> = lhs_node
        .descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect();

    // `PropertyLhs := IDENT` and `SubjectAssign := AT_ATTR ASSIGN Expression`.
    // Two arms, and the three retired spellings that used to reach here — the
    // CURIE, the absolute IRI and the `iri` keyword — do not: the PARSER refuses
    // them now, by name and with a span over the whole form, so a key it could
    // not read never arrives with tokens in it. What is left below is the case
    // the parser cannot classify, which is why the fallback still says what a
    // property name IS rather than only that this is not one.
    let key = match lhs_toks.as_slice() {
        [t] if t.kind() == SyntaxKind::AT_ATTR => {
            if t.text() != "@subject" {
                let name = t.text();
                diagnose(
                    db,
                    &lhs_node,
                    format!(
                        "`{name}` is not something a mapping body declares. The only one is \
                         `@subject = <expr>`, the mapping's identity; `@rename` goes above a \
                         `type` binding, not in a body."
                    ),
                );
                return None;
            }
            PropertyKey::Subject
        }
        [t] if t.kind() == SyntaxKind::IDENT => PropertyKey::Name(SmolStr::from(t.text())),
        // An `ABS_IRI` arm and a `[prefix, sep, local]` CURIE arm lived here,
        // each with its own «write `{short} = …` instead» message. Neither can
        // fire: `ABS_IRI` is not a token and the parser refuses both forms
        // where they are written, which is a better place for the message
        // because it has the source span rather than a `PROPERTY_LHS` that may
        // hold nothing at all.
        _ => {
            diagnose(
                db,
                &lhs_node,
                format!(
                    "`{}` is not a property name. A property is named by a bare name — the last \
                     segment of a predicate IRI the shape declares — and this one is not \
                     written.",
                    lhs_node.text().to_string().trim()
                ),
            );
            return None;
        }
    };

    let Some(expr_node) = node.children().find(|c| c.kind() == SyntaxKind::EXPR) else {
        diagnose(
            db,
            node,
            "this property has no value, so it is not written".to_string(),
        );
        return None;
    };
    // `lower_expr` reports its own refusals — every arm that returns `None`
    // there emits first, which is what makes this `?` silent-free.
    let value = lower_expr(db, &expr_node, types)?;

    Some(HirProperty { key, value })
}

/// Lower an `EXPR` composite node.
///
/// The parser wraps every right-hand side in an `EXPR` with a single child, and
/// we dispatch on that inner kind — [`lower_expr_inner`] has the arms. The three
/// kinds this used to name first, `TEMPLATE_EXPR`, `IRI_EXPR` and
/// `FIELD_REF_EXPR`, are gone with the spellings that built them: there is no
/// backtick, no `<https://…>`, no CURIE and no leading dot.
///
/// # The fallback arm is a diagnostic, not a `None`
///
/// The parser builds fifteen kinds of expression node and this function reads
/// four. Everything else used to reach `_ => None`, and a `None` here means
/// `lower_property` drops the whole property, which `body()` skips without a
/// word — so `slug = str.slug(User.name)` type-checked clean, ran, reported
/// *wrote 1 vertex type*, and emitted a corpus with no `slug` column at all.
/// Measured on 2026-08-06; three programs, two of them silently lossy.
///
/// That is the worst failure mode available to a tool whose stated principle is
/// "if it compiles, it runs", and it happened in the type system's own blind
/// spot: the checker raises nothing because the HIR never saw the expression.
/// The arm now says so. The property is still dropped — closing that needs an
/// HIR form for every expression the grammar admits, which is the standing rule
/// in both directions: a token that does not reach the HIR is a promise the
/// language does not keep, so what the HIR does not need leaves the grammar —
/// but a dropped property is now a red build instead of a quiet hole in the
/// data.
fn lower_expr(
    db: &dyn fossil_base::Db,
    expr_node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirExpr> {
    let Some(inner) = expr_node.children().next() else {
        diagnose(
            db,
            expr_node,
            "there is no expression here, so this property is not written".to_string(),
        );
        return None;
    };
    lower_expr_inner(db, &inner, types)
}

/// Lower an expression node that is already unwrapped from its `EXPR` parent.
///
/// `{{` is how a literal `{` is written — the escape Rust, Python and C# share.
/// The CST keeps the source text verbatim, so
/// resolving it is the HIR's job, and it happens once for every string whether
/// or not that string has a hole.
///
/// **`}}` is NOT an escape here, and that is a departure from the convention
/// those three languages share.** Rust doubles the closing brace because its format
/// grammar gives `}` meaning wherever it appears; ours gives it meaning only
/// after an opener, so a lone `}` in text has exactly one reading and demanding
/// `}}` would reject text nothing was ambiguous about. If that asymmetry ever
/// surprises someone more than the ceremony would have, this is one line.
fn unescape_braces(text: &str) -> SmolStr {
    if text.contains("{{") {
        SmolStr::from(text.replace("{{", "{"))
    } else {
        SmolStr::from(text)
    }
}

/// Split out from [`lower_expr`] because an argument inside an `ARG_LIST` is an
/// expression in its own right: recursion has to start below the wrapper, not
/// above it.
fn lower_expr_inner(
    db: &dyn fossil_base::Db,
    inner: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let inner = inner.clone();
    match inner.kind() {
        SyntaxKind::POSTFIX_EXPR => lower_postfix(db, &inner, types),
        SyntaxKind::BINARY_EXPR => lower_binary(db, &inner, types),
        // Grouping is not a form. `(.a and .b)` means what `.a and .b` means,
        // so the parens are tokens and the one child node is the whole of it.
        //
        // This arm is not a deletion's leftover — it is a hole being closed.
        // `PAREN_EXPR` was parsed and then refused by the fallback below, which
        // meant a language that lowers `and` and `or` could not parenthesise
        // them. A recovery-path `(` with nothing inside leaves no child, and
        // `None` there is right: its diagnostic was already emitted by the
        // parser.
        SyntaxKind::PAREN_EXPR => inner
            .children()
            .next()
            .and_then(|grouped| lower_expr_inner(db, &grouped, types)),
        SyntaxKind::TERNARY_EXPR => lower_ternary(db, &inner, types),
        SyntaxKind::UNARY_EXPR => lower_unary(db, &inner, types),
        // A `PIPELINE_EXPR` arm lived here. `|>` was a second spelling of the
        // member call and ruling 7 of 2026-08-11 retired it; `|>` is not a
        // token, and the parser refuses the operator by name now, so nothing
        // builds the node.
        // NOT MINE — step 6 of `SURFACE-PLAN.md`; removed here only because the
        // kind is already gone from `SyntaxKind` and this file would not compile.
        // A `TEMPLATE_EXPR` arm lived here — a backtick literal the carve left
        // whole, lowered to one literal run. There is no TEMPLATE: the backtick,
        // `${` and `\$` are not tokens, so a string with no hole reaches the
        // `LITERAL_EXPR` arm below as the `STRING` it always was.
        SyntaxKind::INTERP_STRING_EXPR => {
            let mut parts = Vec::new();
            for child in inner.children_with_tokens() {
                match child {
                    fossil_syntax::SyntaxElement::Token(t)
                        if t.kind() == SyntaxKind::STRING_TEXT =>
                    {
                        parts.push(InterpolationPart::Text(unescape_braces(t.text())));
                    }
                    fossil_syntax::SyntaxElement::Node(n)
                        if n.kind() == SyntaxKind::INTERPOLATION =>
                    {
                        // The hole's expression is the one node inside it; the
                        // braces are tokens. A hole that failed to parse leaves
                        // no node, and its diagnostic is already recorded.
                        let hole = n
                            .children()
                            .find_map(|e| lower_expr_inner(db, &e, types))?;
                        parts.push(InterpolationPart::Hole(hole));
                    }
                    _ => {}
                }
            }
            Some(HirExpr::Interpolation(parts))
        }
        // A `FIELD_REF_EXPR` arm lived here — `DOT IDENT`, lowered to
        // `HirExpr::FieldRef`. There is no such node: a
        // leading `.` is refused by the parser, and every reference is
        // qualified, which reaches the `POSTFIX_EXPR` arm as a `ColumnRef`.
        //
        // `HirExpr::FieldRef` itself does NOT go with it: the `idents.len() == 1`
        // case below still builds one for a bare identifier. It is the CST node
        // that is gone, not the HIR form.
        SyntaxKind::LITERAL_EXPR => {
            // A `STRING` literal, a number, or a bare name.
            let toks: Vec<_> = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .filter(|t| {
                    !matches!(
                        t.kind(),
                        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
                    )
                })
                .collect();
            if let Some(s) = toks.iter().find(|t| t.kind() == SyntaxKind::STRING) {
                let raw = s.text();
                let inner_text = raw.trim_start_matches('"').trim_end_matches('"');
                // A string with no hole still spells `{` as `{{`: an escape
                // whose meaning depended on whether the string happened to
                // contain a hole would be a spelling you have to explain twice.
                return Some(HirExpr::StringLit(unescape_braces(inner_text)));
            }
            if let Some(n) = toks.iter().find(|t| t.kind() == SyntaxKind::INTEGER) {
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                return match n.text().parse::<i64>() {
                    Ok(v) => Some(HirExpr::IntLit(v)),
                    Err(e) => {
                        Diagnostic::new(
                            Severity::Error,
                            format!("`{}` is not an integer fossil can carry: {e}", n.text()),
                            span,
                        )
                        .accumulate(db);
                        None
                    }
                };
            }
            if let Some(f) = toks.iter().find(|t| t.kind() == SyntaxKind::FLOAT) {
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                // `1_000.5` is one FLOAT token (grammar.bnf, FLOAT: underscores
                // separate digits in both halves) and `f64::from_str` does not
                // read them, so they come out here rather than being left for
                // whoever parses this text next.
                let digits = f.text().replace('_', "");
                return match digits.parse::<f64>() {
                    Ok(v) => Some(HirExpr::FloatLit(FloatBits::new(v))),
                    Err(e) => {
                        Diagnostic::new(
                            Severity::Error,
                            format!("`{}` is not a float fossil can carry: {e}", f.text()),
                            span,
                        )
                        .accumulate(db);
                        None
                    }
                };
            }
            if let Some(b) = toks.iter().find(|t| t.kind() == SyntaxKind::BOOL) {
                // The token's text is the value: the lexer has one rule per
                // spelling and one kind for both, so `true` is the only text
                // that is not `false`.
                return Some(HirExpr::BoolLit(b.text() == "true"));
            }
            // `IDENT` — a bare name. There is no `(SHAPE_SEP IDENT)?` tail:
            // the CURIE it read is gone — a `:` that is not a mapping header or
            // a ternary is an error — and the parser
            // refuses the spelling before this arm ever sees it, so the
            // two-IDENT branch that expanded a prefix here went with
            // `lookup_prefix`.
            let idents: Vec<_> = toks
                .iter()
                .filter(|t| t.kind() == SyntaxKind::IDENT)
                .collect();
            if idents.len() == 1 {
                // A bare identifier. It reaches the checker as a column of the
                // one row in scope; `HirExpr::ColumnRef` is the qualified
                // spelling and this is what a name with nothing on its left
                // still means.
                Some(HirExpr::FieldRef(SmolStr::from(idents[0].text())))
            } else {
                // Every literal shape this arm knows is handled above. Anything
                // left is a literal the lowering does not read, and a literal it
                // does not read is exactly the silent drop this phase exists to
                // remove: `n = 42` produced no property AND no diagnostic
                // until 2026-08-07.
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                let text = inner.text().to_string();
                Diagnostic::new(
                    Severity::Error,
                    format!(
                        "`{}` is a literal fossil cannot lower yet, so this property will \
                         not be written to the corpus.",
                        text.trim()
                    ),
                    span,
                )
                .accumulate(db);
                None
            }
        }
        // An `IRI_EXPR` arm lived here, 90 lines of it: the `<https://…>` form,
        // the `ex:Local` CURIE, and `${ex:}` — a prefix with no local part,
        // which only the parser's interpolation body admitted. All three
        // spellings are gone — `<` and `>` have one reading each, a `:` that is
        // not a mapping header or a ternary is an error, and a hole takes an
        // expression and nothing else — and so is the node.
        // `HirExpr::PrefixedName` survives them and is now
        // UNREACHABLE from the surface: nothing this file reads constructs one.
        // Deleting the variant reaches `check`, `infer`, `provenance` and
        // `fossil-mir`, so it is its own change.
        other => {
            let range = inner.text_range();
            let span = Span::new(range.start().into(), range.end().into());
            let source = inner.text().to_string();
            let source = source.trim();
            Diagnostic::new(
                Severity::Error,
                format!(
                    "`{source}` is not an expression fossil can lower yet, so this property \
                     will not be written to the corpus. A property value may be a template, \
                     a field reference, a string literal, a prefixed name or a call. \
                     (parsed as {other:?})"
                ),
                span,
            )
            .accumulate(db);
            None
        }
    }
}

/// Lower a `UNARY_EXPR` — `-x` or `not x` (grammar.bnf, UnaryExpr).
///
/// The parser builds the node right-associatively, so `- - x` is a `UNARY_EXPR`
/// wrapping a `UNARY_EXPR`, and this recurses through
/// [`lower_expr_inner`] without knowing that. The operator is the node's own
/// token and the operand is its one child node; a recovery-path unary (an
/// operator with nothing after it) has no child, and returning `None` there is
/// right — the parser has already said so.
///
/// See [`HirExpr::UnaryOp`] for why this is a node rather than a rewrite into
/// `0 - x` / `x == false`.
fn lower_unary(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let op = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find_map(|t| match t.kind() {
            SyntaxKind::MINUS => Some(UnOp::Neg),
            SyntaxKind::KW_NOT => Some(UnOp::Not),
            _ => None,
        })?;

    let operand = lower_expr_inner(db, &node.children().next()?, types)?;
    Some(HirExpr::UnaryOp {
        op,
        operand: Box::new(operand),
    })
}

/// Lower a `TERNARY_EXPR` — `cond ? then : otherwise`.
///
/// The parser leaves three expression children in order and consumes the `?`
/// and the `:` as tokens; a recovery-path ternary (a missing `:`) has an ERROR
/// node among them, and fewer than three expression children means the source
/// is not a ternary the HIR can carry.
fn lower_ternary(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let parts: Vec<_> = node
        .children()
        .filter(|c| c.kind() != SyntaxKind::ERROR)
        .collect();
    if parts.len() != 3 {
        let range = node.text_range();
        let span = Span::new(range.start().into(), range.end().into());
        let source = node.text().to_string();
        Diagnostic::new(
            Severity::Error,
            format!(
                "`{}` is not a complete conditional: it needs a condition, a `?` branch \
                 and a `:` branch.",
                source.trim()
            ),
            span,
        )
        .accumulate(db);
        return None;
    }

    let cond = lower_expr_inner(db, &parts[0], types)?;
    let then = lower_expr_inner(db, &parts[1], types)?;
    let otherwise = lower_expr_inner(db, &parts[2], types)?;
    Some(HirExpr::Ternary {
        cond: Box::new(cond),
        then: Box::new(then),
        otherwise: Box::new(otherwise),
    })
}

/// Lower a `BINARY_EXPR` — a comparison (`.age >= 18`), a boolean connective
/// (`a and b`) or arithmetic (`gross - discount`).
///
/// All three are one node and one arm. Arithmetic used to stop here with a
/// diagnostic saying MIR had no operator to carry `+` into; it has five now, and
/// the operator token is the only thing this function reads.
///
/// **Associativity is the parser's and is not re-decided here.** `gross -
/// discount + shipping` arrives already shaped as `(gross - discount) +
/// shipping` — L5 is left-associative — and taking the first two child nodes in
/// order is what preserves it. Right-associating that expression changes the
/// number, which is why `arithmetic` spells it.
fn lower_binary(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let range = node.text_range();
    let span = Span::new(range.start().into(), range.end().into());
    let source = node.text().to_string();
    let source = source.trim().to_string();

    // The operator is the node's own token; the two operands are its child
    // nodes. A malformed binary node (recovery path) has fewer than two.
    let op_token = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::EQ
                    | SyntaxKind::NEQ
                    | SyntaxKind::LT
                    | SyntaxKind::LE
                    | SyntaxKind::GT
                    | SyntaxKind::GE
                    | SyntaxKind::KW_AND
                    | SyntaxKind::KW_OR
                    | SyntaxKind::PLUS
                    | SyntaxKind::MINUS
                    | SyntaxKind::STAR
                    | SyntaxKind::SLASH
                    | SyntaxKind::PERCENT
            )
        })?;

    let op = match op_token.kind() {
        SyntaxKind::EQ => CmpOp::Eq,
        SyntaxKind::NEQ => CmpOp::Ne,
        SyntaxKind::LT => CmpOp::Lt,
        SyntaxKind::LE => CmpOp::Le,
        SyntaxKind::GT => CmpOp::Gt,
        SyntaxKind::GE => CmpOp::Ge,
        SyntaxKind::KW_AND => CmpOp::And,
        SyntaxKind::KW_OR => CmpOp::Or,
        SyntaxKind::PLUS => CmpOp::Add,
        SyntaxKind::MINUS => CmpOp::Sub,
        SyntaxKind::STAR => CmpOp::Mul,
        SyntaxKind::SLASH => CmpOp::Div,
        SyntaxKind::PERCENT => CmpOp::Rem,
        // Unreachable: the `find` above admits exactly the thirteen kinds
        // matched here. It is a diagnostic and not a panic because the
        // walking-skeleton invariant says the lowering never panics, and a
        // fourteenth operator added to the parser and forgotten here should
        // say so rather than be silently dropped.
        other => {
            Diagnostic::new(
                Severity::Error,
                format!(
                    "`{source}` uses `{}`, which the parser reads as an operator and the \
                     lowering has no case for ({other:?}). This is a compiler gap, not a \
                     mistake in the program.",
                    op_token.text()
                ),
                span,
            )
            .accumulate(db);
            return None;
        }
    };

    let mut operands = node.children();
    let lhs_node = operands.next()?;
    let rhs_node = operands.next()?;
    let lhs = lower_expr_inner(db, &lhs_node, types)?;
    let rhs = lower_expr_inner(db, &rhs_node, types)?;

    Some(HirExpr::BinOp {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    })
}

/// Lower a `POSTFIX_EXPR` — either a call (`str.slug(User.name)`) or the member
/// access that names its callee (`str.slug`).
///
/// The parser builds both with the same node kind, left-associatively: the call
/// node carries an `LPAREN` token and wraps the member-access node, which in
/// turn wraps the base `LITERAL_EXPR`. So "is this a call?" is "does this node
/// hold an `LPAREN`", and the callee's dotted name is read off the chain below.
fn lower_postfix(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let range = node.text_range();
    let span = Span::new(range.start().into(), range.end().into());
    let source = node.text().to_string();
    let source = source.trim().to_string();

    let is_call = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .any(|t| t.kind() == SyntaxKind::LPAREN);

    if !is_call {
        // `orders.user_id` — a qualified column reference, the only kind there
        // is. It reaches here because the CST cannot tell it from a
        // call's callee: both are `IDENT DOT IDENT`. The parenthesis separates
        // those two, and the stdlib catalogue separates this from the case
        // below: `str` is a namespace, `orders` is not.
        if let Some(dotted) = dotted_name(node) {
            let mut parts = dotted.split('.');
            if let (Some(head), Some(column), None) = (parts.next(), parts.next(), parts.next())
                && !crate::stdlib::stdlib().is_catalogued_head(head)
            {
                return Some(HirExpr::ColumnRef {
                    binding: SmolStr::from(head),
                    column: SmolStr::from(column),
                });
            }
        }

        // `name = str.slug` — a function named but never applied. v0.1 has
        // no function values: partial application is not in the grammar, so
        // this is an error, not a value that quietly becomes text.
        Diagnostic::new(
            Severity::Error,
            format!(
                "`{source}` names a function but does not call it. Fossil has no function \
                 values: write `{source}(...)` with its arguments."
            ),
            span,
        )
        .accumulate(db);
        return None;
    }

    let callee = node.children().next()?;

    // ── The VALUE path: `x.trim()` ────────────────────────────────────────
    //
    // `str.trim(x)` and `x.trim()` are ONE catalogue row reached
    // two ways, exactly as `str::len(&s)` ≡ `s.len()` in Rust. The TYPE path is
    // the dotted name below; this is the other one, and it is what
    // `RegistryEntry::member` exists for.
    //
    // It is tried FIRST because the type path cannot express it: a receiver that
    // is itself an expression (`User.email.trim()`) has no dotted name at all,
    // and `dotted_name` would flatten `User.email.trim` into a three-segment
    // string that matches no row.
    //
    // The limit, stated rather than hidden: the receiver's TYPE is not known
    // here, so the member is resolved against the catalogue by NAME. One
    // candidate is a resolution; more than one needs the type and is refused by
    // name rather than guessed. Today no member is spelled on two receivers, so
    // the ambiguous arm is unreachable and is written for the day it is not.
    if let Some(member) = member_name(&callee)
        && let Some(recv_node) = callee.children().next()
        && lower_receiver_is_value(&recv_node)
    {
        let candidates: Vec<&crate::stdlib::RegistryEntry> = crate::stdlib::stdlib()
            .candidates_for_member(member.as_str())
            .collect();
        match candidates.as_slice() {
            [entry] => {
                let name = entry.name.clone();
                let recv = lower_expr_inner(db, &recv_node, types)?;
                // The receiver already occupies parameter 0, so the written
                // arguments start at 1 — which is exactly what a named argument
                // has to know to refuse `x.trim(text = y)` as a value given
                // twice rather than accept it as a second `text`.
                let mut args = vec![recv];
                args.extend(place_args(db, node, types, Some(&entry.sig), &name, 1)?);
                return Some(HirExpr::Call { func: name, args });
            }
            [] => {
                Diagnostic::new(
                    Severity::Error,
                    format!(
                        "`{source}` calls `{member}` on a value, and nothing in the catalogue \
                         has a member called `{member}`."
                    ),
                    span,
                )
                .accumulate(db);
                return None;
            }
            many => {
                Diagnostic::new(
                    Severity::Error,
                    format!(
                        "`{member}` is a member of {} different receivers ({}), and fossil \
                         cannot tell which one this is. Write the type path instead, e.g. \
                         `{}(…)`.",
                        many.len(),
                        many.iter()
                            .map(|e| format!("`{}`", e.name))
                            .collect::<Vec<_>>()
                            .join(", "),
                        many[0].name,
                    ),
                    span,
                )
                .accumulate(db);
                return None;
            }
        }
    }

    let Some(func) = dotted_name(&callee) else {
        Diagnostic::new(
            Severity::Error,
            format!(
                "`{source}` calls something that is not a stdlib function name. Only a \
                 catalogued name may be called, e.g. `str.trim(User.name)`."
            ),
            span,
        )
        .accumulate(db);
        return None;
    };

    // `buyer = Person(User.email)` — an EDGE. A type name is an
    // ordinary `PrimaryExpr` and this is the ordinary call, so the CST is the
    // same one `str.slug(x)` builds; what separates them is whether the name
    // is one a `type { … } := …` binding introduced. Undotted, because a type
    // name has no namespace: `io.shex` is a constructor and `Person` is a type.
    let is_edge = !func.contains('.') && types.iter().any(|t| t == func.as_str());

    // An EDGE has no signature and therefore no parameter names: its arguments
    // fill the target type's identity template BY POSITION, which is the one
    // binding rule the language has (see `HirExpr::Edge`). `None` here is what
    // makes `Person(email = x)` a diagnostic naming that, rather than a lookup
    // that finds nothing.
    let sig = if is_edge {
        None
    } else {
        crate::stdlib::stdlib().lookup(func.as_str()).map(|e| &e.sig)
    };
    let args = place_args(db, node, types, sig, &func, 0)?;

    if is_edge {
        return Some(HirExpr::Edge {
            target: SmolStr::from(func),
            args,
        });
    }
    Some(HirExpr::Call {
        func: SmolStr::from(func),
        args,
    })
}

/// Read a call's `ARG_LIST` into the POSITIONAL argument vector the HIR carries,
/// resolving each `NamedArg := IDENT ASSIGN Expression` against `sig`.
///
/// `offset` is how many parameters are already spoken for before the first
/// written argument: `0` for the type path `parse.date(x, format = f)`, `1` for
/// the value path `x.trim()`, where the receiver is parameter 0.
///
/// # Where the name is resolved, and why here
///
/// See [`crate::stdlib::ParamSpec`]. The short of it: this is the last place
/// that holds both the signature and the source text, so every message below can
/// quote what the author wrote AND list what was available. Nothing downstream
/// learns that named arguments exist — `HirExpr::Call` is positional before this
/// function and positional after it.
///
/// # The four ways it can be wrong, and each one is named
///
/// A name no parameter has (with a did-you-mean), a name for a position already
/// given, a positional argument after a named one, and a hole left between two
/// filled positions. The last is the one that would otherwise be SILENT: without
/// it `parse.csv_row(s, field = 2)` would collapse to two arguments and be
/// reported as an arity error against a call the author did not write.
fn place_args(
    db: &dyn fossil_base::Db,
    call_node: &fossil_syntax::SyntaxNode,
    types: &[SmolStr],
    sig: Option<&crate::stdlib::SigSpec>,
    func: &str,
    offset: usize,
) -> Option<Vec<HirExpr>> {
    use fossil_syntax::SyntaxKind;

    let emit = |node: &fossil_syntax::SyntaxNode, message: String| {
        let r = node.text_range();
        Diagnostic::new(
            Severity::Error,
            message,
            Span::new(r.start().into(), r.end().into()),
        )
        .accumulate(db);
    };

    let Some(list) = call_node
        .children()
        .find(|c| c.kind() == SyntaxKind::ARG_LIST)
    else {
        return Some(Vec::new());
    };

    // Sparse until the end: a named argument writes to its slot and a gap is a
    // fact worth reporting rather than a shift worth hiding.
    let mut slots: Vec<Option<HirExpr>> = Vec::new();
    let put = |slots: &mut Vec<Option<HirExpr>>, at: usize, value: HirExpr| {
        if slots.len() <= at {
            slots.resize_with(at + 1, || None);
        }
        slots[at] = Some(value);
    };
    let mut next_positional = offset;
    let mut first_named: Option<String> = None;

    for arg in list.children() {
        match arg.kind() {
            SyntaxKind::NAMED_ARG => {
                let name = arg
                    .children_with_tokens()
                    .filter_map(fossil_syntax::SyntaxElement::into_token)
                    .find(|t| t.kind() == SyntaxKind::IDENT)?;
                let name = name.text().to_string();
                let Some(sig) = sig else {
                    emit(
                        &arg,
                        format!(
                            "`{}` names an argument of `{func}`, which has no named parameters \
                             to match it against. An edge's arguments fill the target's identity \
                             template in the order they are written.",
                            arg.text().to_string().trim()
                        ),
                    );
                    return None;
                };
                let Some(at) = sig.position_of(&name) else {
                    let available: Vec<&str> =
                        sig.params.iter().map(|p| p.name.as_str()).collect();
                    let suggestion = crate::didyoumean::did_you_mean(&name, available.iter().copied());
                    let list = available
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    emit(
                        &arg,
                        suggestion.map_or_else(
                            || format!("`{func}` has no parameter called `{name}`. It takes {list}."),
                            |s| {
                                format!(
                                    "`{func}` has no parameter called `{name}` — did you mean \
                                     `{s}`? It takes {list}."
                                )
                            },
                        ),
                    );
                    return None;
                };
                if at < offset {
                    emit(
                        &arg,
                        format!(
                            "`{name}` is the receiver of `{func}` — the value to the left of the \
                             dot — so naming it here would give it twice."
                        ),
                    );
                    return None;
                }
                if slots.get(at).is_some_and(Option::is_some) {
                    emit(&arg, format!("`{func}` is given `{name}` twice."));
                    return None;
                }
                let inner = arg
                    .children()
                    .find(|c| c.kind() != SyntaxKind::ERROR)
                    .as_ref()
                    .and_then(|n| lower_expr_inner(db, n, types))?;
                put(&mut slots, at, inner);
                first_named.get_or_insert(name);
            }
            SyntaxKind::ALIAS_ARG => {
                emit(
                    &arg,
                    format!(
                        "`{}` is a source alias, and `{func}` takes values. The alias belongs to \
                         a join over rows.",
                        arg.text().to_string().trim()
                    ),
                );
                return None;
            }
            _ => {
                if let Some(named) = &first_named {
                    emit(
                        &arg,
                        format!(
                            "`{}` is positional and follows `{named} = …`. Once an argument is \
                             named, the ones after it are too — otherwise which position this \
                             fills depends on where the named one landed.",
                            arg.text().to_string().trim()
                        ),
                    );
                    return None;
                }
                let inner = arg.children().next()?;
                // An argument that does not lower has already said why; dropping
                // the whole call keeps the property from being written with a
                // hole in it.
                put(&mut slots, next_positional, lower_expr_inner(db, &inner, types)?);
                next_positional += 1;
            }
        }
    }

    // Positions `0..offset` belong to the receiver and are not this vector's.
    let mut out = Vec::with_capacity(slots.len().saturating_sub(offset));
    for (at, slot) in slots.into_iter().enumerate().skip(offset) {
        match slot {
            Some(e) => out.push(e),
            None => {
                let name = sig
                    .and_then(|s| s.params.get(at))
                    .map_or_else(|| at.to_string(), |p| p.name.to_string());
                emit(
                    &list,
                    format!(
                        "`{func}` is given nothing for `{name}`, and something after it. A \
                         parameter cannot be skipped: name the ones you are giving, or give them \
                         all in order."
                    ),
                );
                return None;
            }
        }
    }
    Some(out)
}

/// The names a file's `type { … } := …` bindings introduce.
///
/// Read once per file and passed down the lowering, because the decision it
/// serves is per-expression and reading it there would mean a `DefMap` lookup
/// per call node. Order is source order; duplicates are possible (two bindings
/// may introduce the same name) and harmless — this is a membership test.
fn bound_type_names(db: &dyn fossil_base::Db, dm: crate::def_map::DefMap<'_>) -> Vec<SmolStr> {
    dm.types(db).iter().map(|t| t.name.clone()).collect()
}

/// The dotted name a callee chain spells: `str.slug` → `"str.slug"`.
///
/// Returns `None` for anything that is not a plain name — `f(x).y`, an
/// interpolated string, a literal. The catalog is keyed by these strings, so a callee that
/// cannot produce one cannot be looked up.
fn dotted_name(node: &fossil_syntax::SyntaxNode) -> Option<String> {
    use fossil_syntax::SyntaxKind;

    let idents = |n: &fossil_syntax::SyntaxNode| -> Vec<String> {
        n.children_with_tokens()
            .filter_map(fossil_syntax::SyntaxElement::into_token)
            .filter(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| t.text().to_string())
            .collect()
    };

    match node.kind() {
        SyntaxKind::LITERAL_EXPR => {
            let ids = idents(node);
            (ids.len() == 1).then(|| ids[0].clone())
        }
        SyntaxKind::POSTFIX_EXPR => {
            // A member access: the base chain, then this node's own IDENT.
            let has_paren = node
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .any(|t| t.kind() == SyntaxKind::LPAREN);
            if has_paren {
                return None; // `f(x).y` — the base is a call, not a name
            }
            let base = dotted_name(&node.children().next()?)?;
            let ids = idents(node);
            (ids.len() == 1).then(|| format!("{base}.{}", ids[0]))
        }
        _ => None,
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name = User.name
";

    fn lower_src(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    /// `users.name` lowers to a qualified column reference, the only kind the
    /// language has. It arrives at the parser as `IDENT DOT IDENT` — the very
    /// shape of a call's callee — so this pins the branch that separates them.
    #[test]
    fn a_qualified_reference_lowers_to_a_column_ref() {
        let (db, file) = lower_src(
            "type { Person } := io.shex(\"personas.shex\")\n\nUser := io.csv(\"u.csv\")\n\nUsers : Person from User\n    name = User.name\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        assert_eq!(
            body.properties(&db)[0].value,
            HirExpr::ColumnRef {
                binding: "users".into(),
                column: "name".into()
            }
        );
    }

    /// And the diagnostic it shares a CST shape with SURVIVES: `str.slug` is
    /// a catalogued namespace, so it is still a function named but not applied,
    /// not a column of a row called `clean`. The stdlib catalogue is what tells
    /// the two apart.
    #[test]
    fn a_stdlib_name_without_its_call_is_still_an_error() {
        let (db, file) = lower_src(
            "type { Person } := io.shex(\"personas.shex\")\n\nUser := io.csv(\"u.csv\")\n\nUsers : Person from User\n    name = str.slug\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        assert!(
            body.properties(&db).is_empty(),
            "a function named but not called must not lower to a value"
        );
    }

    /// The spelling that replaced the backtick: a plain string, interpolated
    /// with `{expr}`, resolved at compile time.
    ///
    /// The IRI is written in full — no `base`, no CURIE with holes — and the
    /// hole is a QUALIFIED reference, which is only expressible because the
    /// hole is parsed by the expression parser. The old `${…}` scan understood
    /// exactly two shapes and echoed anything else back as text.
    #[test]
    fn a_quoted_string_interpolates_and_its_hole_is_an_expression() {
        let (db, file) = lower_src(
            "type { Person } := io.shex(\"personas.shex\")\n\nUser := io.csv(\"u.csv\")\n\nUsers : Person from User\n    @subject = \"https://example.org/user/{User.id}\"\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        let HirExpr::Interpolation(parts) = &body.properties(&db)[0].value else {
            panic!(
                "expected an interpolation, got {:?}",
                body.properties(&db)[0].value
            );
        };
        assert_eq!(
            parts,
            &vec![
                InterpolationPart::Text("https://example.org/user/".into()),
                InterpolationPart::Hole(HirExpr::ColumnRef {
                    binding: "users".into(),
                    column: "id".into(),
                }),
            ]
        );
    }

    /// `{{` is a literal brace whether or not the string has a hole — an escape
    /// whose meaning depended on that would be a spelling you explain twice
    /// (house rule 2) — and `}` needs no escape at all, because outside a hole
    /// it has only one reading. See `unescape_braces` for why that asymmetry is
    /// deliberate rather than half a convention copied badly.
    #[test]
    fn a_doubled_brace_is_one_brace_and_a_closing_brace_needs_no_escape() {
        let (db, file) = lower_src(
            "type { Person } := io.shex(\"personas.shex\")\n\nUser := io.csv(\"u.csv\")\n\nUsers : Person from User\n    a = \"{{literal}\"\n    b = \"{{x}{users.id}\"\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        let props = body.properties(&db);
        assert_eq!(
            props[0].value,
            HirExpr::StringLit("{literal}".into()),
            "a string with no hole still resolves `{{{{`, and keeps a lone `}}`"
        );
        let HirExpr::Interpolation(parts) = &props[1].value else {
            panic!("expected an interpolation, got {:?}", props[1].value);
        };
        assert_eq!(
            parts.first(),
            Some(&InterpolationPart::Text("{x}".into())),
            "and so does one that has a hole after it"
        );
    }

    /// There is ONE subject spelling, and the call shape is not it.
    ///
    /// This test used to prove that `iri = …` and `@subject(iri = …)` lowered
    /// identically, so that a fixture rewrite would change spelling without
    /// changing meaning. Both are gone: the identity is an
    /// ASSIGNMENT, and `a0d9bfa`'s call shape lived two hours. What is worth
    /// pinning now is that the retired form is an ERROR rather than a property
    /// that quietly vanishes — `@subject(...)` parses its `(` as the start of
    /// nothing, and a mapping whose identity did not lower is reported by
    /// `check_identity`.
    #[test]
    fn the_call_shaped_subject_is_gone_and_says_so() {
        const HEAD: &str = "type { Person } := io.shex(\"personas.shex\")\n\nUser := io.csv(\"u.csv\")\n\nUsers : Person from User\n";
        let (db, file) = lower_src(&format!(
            "{HEAD}    @subject(iri = \"https://example.org/u/{{users.id}}\")\n"
        ));
        let m = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let props = crate::body::body(&db, m).properties(&db);
        let diags = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, m);
        assert!(
            diags.iter().any(|d| d.message.contains("expected ASSIGN")),
            "`@subject` takes `=`, not `(`, got: {props:#?} / {diags:#?}"
        );
    }

    /// `@subject` is the FIRST line of a body, and the second one is not a
    /// second identity — a type has exactly one, and it is declared once.
    #[test]
    fn the_identity_is_required_once_and_first() {
        const HEAD: &str = "type { Person } := io.shex(\"personas.shex\")\n\nUser := io.csv(\"u.csv\")\n\nUsers : Person from User\n";
        let msgs = |src: &str| {
            let (db, file) = lower_src(src);
            let m = crate::def_map::def_map(&db, file).mappings(&db)[0];
            let _ = crate::body::body(&db, m);
            crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, m)
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
        };
        assert!(
            msgs(&format!("{HEAD}    name = User.name\n"))
                .iter()
                .any(|m| m.contains("declares no `@subject`")),
            "an identity is required"
        );
        assert!(
            msgs(&format!(
                "{HEAD}    @subject = \"a\"\n    @subject = \"b\"\n"
            ))
            .iter()
            .any(|m| m.contains("declares `@subject` twice")),
            "there is exactly one"
        );
        assert!(
            msgs(&format!("{HEAD}    name = User.name\n    @subject = \"a\"\n"))
                .iter()
                .any(|m| m.contains("is the first line of a mapping body")),
            "and it comes first"
        );
    }

    /// The sigil now parses, so an unknown one must be an ERROR and not a
    /// property that quietly disappears — the failure this file's fallback arm
    /// exists to prevent.
    #[test]
    fn an_unknown_attribute_is_a_diagnostic_and_not_a_dropped_property() {
        let (db, file) = lower_src(
            "type { Person } := io.shex(\"personas.shex\")\n\nUser := io.csv(\"u.csv\")\n\nUsers : Person from User\n    @sensitive(iri = \"x\")\n    name = User.name\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        assert_eq!(
            body.properties(&db).len(),
            1,
            "the good property survives; only the bad attribute drops"
        );
        let diags = crate::body::body::accumulated::<Diagnostic>(&db, mapping);
        assert!(
            diags.iter().any(|d| d.message.contains("@sensitive")),
            "the diagnostic must name the attribute, got {diags:?}"
        );
    }

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    /// The call that used to be a hole is now a `Call` in the HIR.
    ///
    /// Until 2026-08-07 this exact program was the regression fixture for a
    /// measured data-loss bug: `slug = str.slug(User.name)` was dropped from
    /// `HirBody` in silence, `fossil check` said *ok*, `fossil run` said *wrote
    /// 1 vertex type*, and the column was absent from the corpus. Then the drop
    /// was made loud. This is the same program with the hole closed: the
    /// property survives lowering, carrying the function's name and its
    /// argument, and no diagnostic is raised at all.
    #[test]
    fn a_call_lowers_to_a_call_and_raises_nothing() {
        const CALLS_A_BUILTIN: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    slug = str.slug(User.name)
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, CALLS_A_BUILTIN.to_string(), "t.fossil".to_string());

        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        let diagnostics = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diagnostics.is_empty(),
            "a call the lowering understands must raise nothing, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );

        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 2, "both properties survive lowering");
        let HirExpr::Call { func, args } = &props[1].value else {
            panic!("expected a Call, got {:?}", props[1].value);
        };
        assert_eq!(func.as_str(), "str.slug");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], HirExpr::FieldRef(SmolStr::from("name")));
    }

    /// A comparison lowers, with its integer literal.
    ///
    /// The literal is half the point: until 2026-08-07 `n = 42` was dropped
    /// **with no diagnostic at all** — the 2026-08-06 fix made unknown node
    /// KINDS loud, and a `LITERAL_EXPR` holding an integer is a known kind
    /// whose arm returned `None`. A second silent hole in the same blind spot,
    /// found by needing a right-hand side for this test.
    #[test]
    fn a_comparison_lowers_with_its_integer_literal() {
        const COMPARES: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    adult = .age >= 18
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, COMPARES.to_string(), "t.fossil".to_string());
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        let diagnostics = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diagnostics.is_empty(),
            "a comparison the lowering understands must raise nothing, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 2);
        let HirExpr::BinOp { op, lhs, rhs } = &props[1].value else {
            panic!("expected a BinOp, got {:?}", props[1].value);
        };
        assert_eq!(*op, CmpOp::Ge);
        assert_eq!(**lhs, HirExpr::FieldRef(SmolStr::from("age")));
        assert_eq!(**rhs, HirExpr::IntLit(18));
    }

    /// Grouping is not a form, and the parentheses leave no trace.
    ///
    /// `PAREN_EXPR` had no arm in the lowering, so a parenthesised expression
    /// was refused by the fallback diagnostic — in a language that already
    /// lowers `and` and `or`, which are exactly what you reach for parentheses
    /// to group. This is the hole closed: the tree is the tree the same source
    /// without parens would have produced.
    #[test]
    fn parentheses_group_and_then_disappear() {
        const GROUPED: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    adult = (.age >= 18)
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, GROUPED.to_string(), "t.fossil".to_string());
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        let diagnostics = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diagnostics.is_empty(),
            "`(.age >= 18)` must lower without a word, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 2);
        assert_eq!(
            props[1].value,
            HirExpr::BinOp {
                op: CmpOp::Ge,
                lhs: Box::new(HirExpr::FieldRef(SmolStr::from("age"))),
                rhs: Box::new(HirExpr::IntLit(18)),
            },
            "the parens must leave no trace in the HIR",
        );
    }

    /// The loud-drop guarantee, re-pinned on a form that is still a hole.
    ///
    /// `call` (F2 §1) and `comparison` (F2 §2) have landed; `conditional` and
    /// `pipeline` follow, and arithmetic has no MIR operator to be carried
    /// into. Until then a property whose value is one of them is still dropped
    /// — and this asserts the drop stays **loud**, which is the difference
    /// between a known limitation and silent corruption. The day there is no
    /// form left, this test is deleted, not weakened.
    #[test]
    fn an_unlowerable_expression_is_a_diagnostic_and_not_a_silent_drop() {
        const ARITHMETIC: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    doble = .id * 2
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, ARITHMETIC.to_string(), "t.fossil".to_string());

        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        let diagnostics = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            !diagnostics.is_empty(),
            "an expression the lowering cannot read must produce a diagnostic",
        );
        let d = &diagnostics[0];
        assert_eq!(d.severity, fossil_base::Severity::Error);
        assert!(
            d.message.contains(".id * 2"),
            "the diagnostic must quote what the user wrote, got: {}",
            d.message,
        );
        assert!(
            d.span.end > d.span.start,
            "the diagnostic must point somewhere, got {:?}",
            d.span,
        );

        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 1, "the unlowerable property is still dropped");
    }

    /// The same guarantee, on the three places that were still silent.
    ///
    /// An undeclared prefix WAS one missing `prefix` line — there are no
    /// `prefix` lines and no CURIEs in the language now, so the fixture below
    /// spells forms the parser refuses outright, and what it still pins is that
    /// none of the three positions drops anything in silence. The lowering
    /// answered it with a bare `?` in three of the five places it looked one up:
    /// in a mapping's header the WHOLE MAPPING disappeared from the HIR, and in
    /// a property key or a CURIE value the property did. The other two already
    /// reported it, and one of them carries a comment naming "the legacy
    /// `LITERAL_EXPR` branch's silent-drop behaviour" — the branch three lines
    /// from this fix. `lower_property_public`'s own doc comment claimed the key
    /// position reported it, and that was false for as long as the comment
    /// existed.
    ///
    /// This is the failure mode a corpus rewrite turns into a green, empty
    /// suite: nothing fails, and the data is simply not there.
    #[test]
    fn an_undeclared_prefix_is_a_diagnostic_and_not_a_silent_drop() {
        // No `prefix ex:` line anywhere: the header, the property key and the
        // CURIE value each name one that is not declared.
        const NO_PREFIX: &str = "\
users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    @subject = \"x\"
    kind = ex:Human
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, NO_PREFIX.to_string(), "t.fossil".to_string());

        // The header: the mapping vanishes from the HIR, and now says so.
        let hir = lower_to_hir(&db, file);
        assert!(
            hir.mappings(&db).is_empty(),
            "the header names an undeclared prefix, so the mapping is not lowered"
        );
        let header_diags = lower_to_hir::accumulated::<fossil_base::Diagnostic>(&db, file);
        assert!(
            header_diags
                .iter()
                .any(|d| d.message.contains("undeclared prefix `ex:`")
                    && d.message.contains("not compiled")),
            "a mapping that disappears must say why, got: {header_diags:#?}"
        );

        // The body: only the CURIE VALUE is left to drop. The key half of this
        // test went with the CURIE key — a bare name has no prefix to fail to
        // find — so what used to be two silent drops is one.
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one MAPPING node");
        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 1, "`kind = ex:Human` drops, got {props:#?}");
        let body_diags = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        let prefix_diags: Vec<_> = body_diags
            .iter()
            .filter(|d| d.message.contains("undeclared prefix `ex:`"))
            .collect();
        assert_eq!(
            prefix_diags.len(),
            1,
            "the dropped CURIE value is reported, got: {body_diags:#?}"
        );
        for d in &prefix_diags {
            assert_eq!(d.severity, fossil_base::Severity::Error);
            assert!(d.span.end > d.span.start, "got {:?}", d.span);
        }
    }

    /// `<https://example.org/name> = .name` is no longer a property name.
    ///
    /// It was one — the key position had no arm for it and fell through to a
    /// silent `None`, and the arm that fixed that is now gone with the form.
    /// There is one spelling left for a key, a bare name, and the message
    /// has to say which bare name: an author who wrote the IRI knows the IRI,
    /// and the last segment is the thing they now write instead.
    #[test]
    fn an_absolute_iri_is_not_a_property_name_and_the_message_says_what_is() {
        const ABS: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"x\"
    <https://example.org/name> = .name
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, ABS.to_string(), "t.fossil".to_string());
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");
        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 1, "only the identity lowers, got {props:#?}");
        let diags = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("not by an absolute IRI")
                    && d.message.contains("`name = …`")),
            "the message must name the bare key to write instead, got: {diags:#?}"
        );
    }

    /// A name that is not catalogued is a type error, not a lowering hole: the
    /// HIR carries the call, and the checker is what refuses it.
    #[test]
    fn an_uncatalogued_function_lowers_and_the_checker_refuses_it() {
        const UNKNOWN_FN: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    slug = str.sluggify(User.name)
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, UNKNOWN_FN.to_string(), "t.fossil".to_string());
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        assert_eq!(
            crate::body::body(&db, mloc).properties(&db).len(),
            2,
            "lowering carries the call; resolving the name is the checker's job"
        );
        let diags =
            crate::check::typecheck_mapping::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diags.iter().any(|d| d.message.contains("str.sluggify")
                && d.message.contains("did you mean")),
            "the checker must name the function and suggest one, got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn lower_hello_produces_one_mapping_header() {
        let (db, file) = db_with_hello();
        let hir = lower_to_hir(&db, file);
        let mappings = hir.mappings(&db);
        assert_eq!(mappings.len(), 1);
        let m = &mappings[0];
        assert_eq!(m.name.as_str(), "User");
        assert_eq!(m.shape_iri.as_str(), "https://example.org/Person");
        assert_eq!(m.source_binding.as_str(), "users");
    }

    /// Body content (the property list) now lives behind the
    /// `body(db, MappingLoc)` Salsa query. Phase 1's `mappings[0].properties`
    /// access is replaced by `body(db, def_map.mappings()[0]).properties(db)`.
    #[test]
    fn lower_hello_body_has_two_properties() {
        let (db, file) = db_with_hello();
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("hello has one mapping");
        let body = crate::body::body(&db, mloc);
        let props = body.properties(&db);
        assert_eq!(props.len(), 2);
    }

    #[test]
    fn lower_hello_property_zero_is_iri_template() {
        let (db, file) = db_with_hello();
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("hello has one mapping");
        let body = crate::body::body(&db, mloc);
        let p0 = &body.properties(&db)[0];
        assert!(matches!(p0.key, PropertyKey::Subject));
        // The subject is parts now, not text: the prefix resolved to its IRI at
        // lowering time and the hole is a `FieldRef` node, so this asserts on
        // the tree rather than on a substring of the token.
        match &p0.value {
            HirExpr::Interpolation(parts) => {
                assert_eq!(
                    parts.first(),
                    Some(&InterpolationPart::Hole(HirExpr::PrefixedName {
                        iri: "https://example.org/".into()
                    })),
                    "the prefix hole resolves to the prefix IRI, in HIR"
                );
                assert!(
                    parts.iter().any(|p| matches!(
                        p,
                        InterpolationPart::Hole(HirExpr::FieldRef(f)) if f == "id"
                    )),
                    "the field hole is a field reference, got {parts:?}"
                );
            }
            other => panic!("expected an interpolation, got {other:?}"),
        }
    }

    #[test]
    fn lower_hello_property_one_is_prefixed_name_field_ref() {
        let (db, file) = db_with_hello();
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("hello has one mapping");
        let body = crate::body::body(&db, mloc);
        let p1 = &body.properties(&db)[1];
        match &p1.key {
            PropertyKey::Name(name) => {
                assert_eq!(name.as_str(), "name");
            }
            PropertyKey::Subject => panic!("expected a named key, got the identity"),
        }
        match &p1.value {
            HirExpr::FieldRef(f) => assert_eq!(f.as_str(), "name"),
            other => panic!("expected FieldRef value, got {other:?}"),
        }
    }

    #[test]
    fn lower_to_hir_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let a = lower_to_hir(&db, file);
        let b = lower_to_hir(&db, file);
        assert_eq!(a, b);
    }

    // ===== Plan 03-01 Task 2: `IRI_EXPR` prefixed-name arm =====

    /// Fixture that exercises the `IRI_EXPR` prefixed-name RHS form
    /// (`link = ex:Foo`). Pre-plan-03-01 this property was silently
    /// dropped from `HirBody.properties` — see the `deferred-items.md`
    /// under `.planning/phases/02-full-grammar-hir-foundation/`.
    const HELLO_WITH_IRI_RHS: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    link = ex:Foo
";

    /// Same source as [`HELLO_WITH_IRI_RHS`] but with prefix `ex:` REPLACED
    /// by `nope:` on the RHS — so the prefix `nope:` is undeclared. The
    /// LHS keeps `ex:` so the property's key still parses; only the value
    /// fails prefix resolution. Validates the undeclared-prefix diagnostic
    /// path without confounding the test by also breaking the LHS.
    const HELLO_WITH_UNKNOWN_PREFIX_RHS: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    link = nope:Foo
";

    /// Plan 03-01 Task 2 — happy path: `link = ex:Foo` no longer
    /// silently drops. The property appears in `body.properties()` with
    /// a `HirExpr::PrefixedName { iri: "https://example.org/Foo" }` value.
    /// This is the structural fix the deferred-items.md flagged.
    #[test]
    fn iri_expr_lowers_prefixed_name_form() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_WITH_IRI_RHS.to_string(),
            "iri_rhs.fossil".to_string(),
        );
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");
        let body = crate::body::body(&db, mloc);
        let props = body.properties(&db);

        assert_eq!(
            props.len(),
            2,
            "link = ex:Foo must NOT be silently dropped — \
             expected 2 properties (iri + ex:link), got {}: {:?}",
            props.len(),
            props
        );

        // The second property is `link = ex:Foo`.
        let p1 = &props[1];
        match &p1.key {
            PropertyKey::Name(name) => {
                assert_eq!(name.as_str(), "link", "the LHS key is a bare name");
            }
            PropertyKey::Subject => panic!("expected a named LHS, got the identity"),
        }
        match &p1.value {
            HirExpr::PrefixedName { iri } => {
                assert_eq!(
                    iri.as_str(),
                    "https://example.org/Foo",
                    "RHS prefixed-name must resolve to full IRI via prefix table"
                );
            }
            other => panic!("expected HirExpr::PrefixedName for RHS `ex:Foo`, got {other:?}"),
        }
    }

    /// Plan 03-01 Task 2 — error path: an undeclared prefix on the RHS
    /// (`link = nope:Foo`) emits a diagnostic via the Salsa accumulator
    /// AND still drops the property (matches the rest of `lower_expr`'s
    /// silent-None convention; the `IRI_EXPR` branch is the only one that
    /// adds the diagnostic emit on top).
    #[test]
    fn iri_expr_unknown_prefix_emits_diagnostic() {
        use fossil_base::Diagnostic;

        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_WITH_UNKNOWN_PREFIX_RHS.to_string(),
            "unknown_prefix_rhs.fossil".to_string(),
        );
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        // Drive the body() Salsa query so the accumulator fires.
        let body = crate::body::body(&db, mloc);
        let props = body.properties(&db);
        // `link = nope:Foo` is still dropped (the diagnostic does not
        // prevent the outer property's `?` from short-circuiting). Only
        // the `iri = template` property remains.
        assert_eq!(
            props.len(),
            1,
            "undeclared-prefix RHS still drops the property (silent-None \
             convention), got {} properties",
            props.len()
        );

        // The diagnostic IS emitted via the accumulator, keyed on the
        // body() query that triggered the lowering.
        let diags = crate::body::body::accumulated::<Diagnostic>(&db, mloc);
        assert!(
            !diags.is_empty(),
            "undeclared RHS prefix `nope:` MUST emit at least one Diagnostic \
             (not silent drop)"
        );
        let msg = &diags[0].message;
        assert!(
            msg.contains("nope"),
            "diagnostic must name the offending prefix `nope`, got {msg:?}"
        );
        assert!(
            msg.contains("undeclared") || msg.contains("undefined") || msg.contains("unknown"),
            "diagnostic must say the prefix is undeclared, got {msg:?}"
        );
    }

    fn db_with(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "t.fossil".to_string());
        (db, file)
    }

    /// A source pipeline lowers to its verbs, in the order they were written,
    /// and a binding that reads a file does not become one.
    ///
    /// The spine is a `POSTFIX_EXPR` chain that nests to the LEFT, so this is
    /// also the test that it is walked the right way round: `join` then `where`,
    /// not the reverse. It was `a |> f() |> g()` and the shape of the tree is
    /// the only thing about it that changed.
    #[test]
    fn a_source_pipeline_lowers_to_its_verbs_in_written_order() {
        const PIPES: &str = "\
User := io.csv(\"u.csv\")
Person := io.csv(\"p.csv\")
Adults := User.where(User.age >= 18)
Brief := Adults.select(User.id, User.name)
Sales := Adults.join(Person, on = User.person_id == Person.id).where(User.total >= 100)
";
        let (db, file) = db_with(PIPES);
        let hir = lower_to_hir(&db, file);
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        assert!(
            diags.is_empty(),
            "the three verbs raise nothing, got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );

        let pipes = hir.source_pipes(&db);
        // The two `io.csv` bindings are NOT pipelines. They build the very same
        // CST shape — a call whose callee is a member access over a bare name —
        // and what tells them apart is that `io` is a catalogued head.
        assert_eq!(
            pipes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["Adults", "Brief", "Sales"],
        );

        assert_eq!(pipes[0].base.as_str(), "User");
        assert!(matches!(pipes[0].ops.as_slice(), [HirSourceOp::Where(_)]));

        assert_eq!(pipes[1].base.as_str(), "Adults");
        let [HirSourceOp::Select(cols)] = pipes[1].ops.as_slice() else {
            panic!("expected one Select, got {:?}", pipes[1].ops);
        };
        assert_eq!(
            cols.iter().map(SmolStr::as_str).collect::<Vec<_>>(),
            ["id", "name"]
        );

        assert_eq!(pipes[2].base.as_str(), "Adults");
        let [HirSourceOp::Join { right, alias, on }, HirSourceOp::Where(pred)] =
            pipes[2].ops.as_slice()
        else {
            panic!("expected Join then Where, got {:?}", pipes[2].ops);
        };
        assert_eq!(right.as_str(), "Person");
        assert!(alias.is_none(), "no `as` was written");
        // The condition is a PREDICATE relating two QUALIFIED columns — the
        // whole of ruling 17's first half. It was a bare `on = .k`.
        let HirExpr::BinOp {
            op: CmpOp::Eq,
            lhs,
            rhs,
        } = on
        else {
            panic!("the join condition must be an equality, got {on:?}");
        };
        assert!(matches!(
            (&**lhs, &**rhs),
            (
                HirExpr::ColumnRef { .. },
                HirExpr::ColumnRef { .. }
            )
        ));
        assert!(matches!(pred, HirExpr::BinOp { op: CmpOp::Ge, .. }));
    }

    /// **The self-join.** `Node.join(Node as Other, on = …)` — the alias is the
    /// only thing that can tell the two sides apart once both are the same
    /// binding, and `HirSourceOp::Join` had nowhere to put it
    /// (`grammar.bnf, AliasArg` specified it; the HIR did not carry it).
    #[test]
    fn a_self_join_carries_its_alias() {
        const SELF_JOIN: &str = "\
Node := io.csv(\"n.csv\")
Tree := Node.join(Node as Other, on = Node.parent == Other.id)
";
        let (db, file) = db_with(SELF_JOIN);
        let hir = lower_to_hir(&db, file);
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        assert!(
            diags.is_empty(),
            "a self-join raises nothing, got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );

        let pipes = hir.source_pipes(&db);
        let [pipe] = pipes.as_slice() else {
            panic!("expected one pipeline, got {pipes:?}");
        };
        let [HirSourceOp::Join { right, alias, .. }] = pipe.ops.as_slice() else {
            panic!("expected one Join, got {:?}", pipe.ops);
        };
        assert_eq!(right.as_str(), "Node");
        assert_eq!(
            alias.as_ref().map(SmolStr::as_str),
            Some("Other"),
            "the alias is what distinguishes the two sides"
        );
    }

    /// **THE PROGRAM WHOSE MEANING CHANGED**, and the only one in this phase.
    ///
    /// Two sources with a column of the SAME NAME. This was rejected — the
    /// message was «would give one row two columns called `name`… rename one
    /// side before joining» — and it is legal now (ruling 17). The rule was
    /// removed rather than relaxed, because it was standing in for an ambiguity
    /// that qualification removes: the body writes `User.name` and
    /// `Person.name`, and each says which row it means.
    ///
    /// The test asserts on the DIAGNOSTIC, not on the row type: what changed is
    /// that the program is accepted.
    #[test]
    fn two_sources_with_a_column_of_the_same_name_now_join() {
        const COLLIDING: &str = "\
User := io.csv(\"u.csv\")
Person := io.csv(\"p.csv\")
Both := User.join(Person, on = User.person_id == Person.id)
";
        let (db, file) = db_with(COLLIDING);
        let hir = lower_to_hir(&db, file);
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        assert!(
            diags.is_empty(),
            "a shared column name is no longer an error, got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
        assert_eq!(hir.source_pipes(&db).len(), 1, "the join must lower");
    }

    /// A `join` with no `on` is named, not guessed — and the message shows the
    /// predicate form, which is the one that survived.
    #[test]
    fn a_join_without_a_condition_is_a_diagnostic() {
        const NO_ON: &str = "\
Order := io.csv(\"o.csv\")
Person := io.csv(\"p.csv\")
Sales := Order.join(Person)
";
        let (db, file) = db_with(NO_ON);
        let hir = lower_to_hir(&db, file);
        assert!(
            hir.source_pipes(&db).is_empty(),
            "the pipeline must not lower, got {:?}",
            hir.source_pipes(&db),
        );
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        let msg = diags.first().map(|d| d.message.clone()).unwrap_or_default();
        assert!(
            msg.contains("on = <predicate>"),
            "the diagnostic must show the form that works, got {msg:?}"
        );
    }

    /// A verb the lowering does not implement is named, not ignored — and the
    /// message counts the catalogue's relation members, so the gap between what
    /// a relation HAS and what the lowering DOES is visible.
    #[test]
    fn an_unknown_source_verb_is_a_diagnostic() {
        const UNKNOWN: &str = "\
User := io.csv(\"u.csv\")
Odd := User.regroup(User.age)
";
        let (db, file) = db_with(UNKNOWN);
        let hir = lower_to_hir(&db, file);
        assert!(hir.source_pipes(&db).is_empty());
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        let msg = diags.first().map(|d| d.message.clone()).unwrap_or_default();
        assert!(
            msg.contains("regroup") && msg.contains("where"),
            "the diagnostic must name the verb and the ones that exist, got {msg:?}"
        );
    }

    /// **The value path** — `x.trim()` is the same catalogue row `str.trim(x)`
    /// is — one row, two spellings — and this is the test that the lowering
    /// resolves it.
    #[test]
    fn a_member_call_on_a_value_lowers_to_the_catalogue_row() {
        const SRC: &str = "\
type { Person } := io.shex(\"p.shex\")

User := io.csv(\"u.csv\")

Users : Person from User
    @subject = \"http://example.org/{User.id}\"
    name = User.name.trim()
";
        let (db, file) = db_with(SRC);
        let mapping = crate::MappingLoc::new(&db, file, 0);
        let b = crate::body::body(&db, mapping);
        let call = b
            .properties(&db)
            .iter()
            .find_map(|p| match &p.value {
                HirExpr::Call { func, args } => Some((func.clone(), args.clone())),
                _ => None,
            })
            .expect("`User.name.trim()` must lower to a Call");
        assert_eq!(
            call.0.as_str(),
            "str.trim",
            "the value path must reach the `str.trim` row"
        );
        assert_eq!(call.1.len(), 1, "the receiver becomes argument 0");
        assert!(matches!(call.1[0], HirExpr::ColumnRef { .. }));
    }
}
