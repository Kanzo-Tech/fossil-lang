//! The Fossil v0.1 standard-library catalog — every function the language
//! declares, with its receiver, its signature and how it compiles.
//!
//! # Why it lives in `fossil-hir`
//!
//! The checker resolves a `HirExpr::Call` against this catalog: arity, argument
//! types and the result type all come from here. It used to be its own crate,
//! `fossil-registry`, which depended on `fossil-hir` for `FnSig` — so the
//! checker could not read it without a cycle, and for as long as that held, a
//! call could not be typed at all. The catalog is language surface, so it moved
//! down, not the checker up.
//!
//! # A row is DATA: receiver + name + signature + lowering
//!
//! Ruling 14 of `SURFACE-PLAN.md`. The catalogue is on its way to being a file
//! the compiler reads, so a row carries nothing a file could not: four fields,
//! all of them values. `grammar.bnf` then says the SHAPE of a program and this
//! says WHICH NAMES EXIST — two data files and a compiler. Adding
//! `str.slugify` over `regexp_replace` stops touching Rust.
//!
//! Nothing here loads a file yet, and that is deliberate: what this module owes
//! the loader is a shape it can fill, not a parser it has to agree with.
//!
//! ## The receiver is the field that ends dispatch-by-string
//!
//! `RegistryEntry::name` used to be the whole of the key — the string
//! `"clean.trim"` — and dispatch by RECEIVER TYPE exists to replace exactly
//! that. A row now says what it hangs off
//! (`Receiver`) and what it is called after the dot
//! (`RegistryEntry::member`), and both are derived in ONE place,
//! `split_receiver`, from the dotted name. They are not constructor
//! arguments: a field a call site fills by hand is a field that ends up
//! disagreeing with the name beside it.
//!
//! Three ways in, and each is a different question:
//!
//! - `FunctionRegistry::lookup` — the TYPE path, `str.trim(x)`. By dotted
//!   name, which is what a `HirExpr::Call` carries.
//! - `FunctionRegistry::lookup_member` — the VALUE path, `x.trim()`. By
//!   receiver and member, which is what a postfix call knows.
//! - `FunctionRegistry::members_of` — every member of a receiver. This is
//!   what makes completion stop being approximate: the IDE can ask what a
//!   value HAS instead of offering the whole catalogue.
//!
//! ## Two lowerings, and no third
//!
//! Ruling 15. `LoweringKind` had four variants and `InlineForm` another nine,
//! and the nine were SQL templates whose text was already written in their own
//! doc-comments — the datum existed, in a comment, where nothing could execute
//! it. Two of them (`SplitPart`, `JsonExtract`) were literally what `Builtin`
//! was: a call by name. The `Builtin`/`Inline` border was not semantic.
//!
//! What is left is the one border that is: `LoweringKind::Expr` is a scalar
//! SQL expression and `LoweringKind::Op` names an operator of the algebra, a
//! closed set of 14. `Udf` is gone — see `LoweringKind::Expr` for what
//! that cost and bought.
//!
//! ## `WasmClass` is gone as a CONCEPT
//!
//! There was a `WasmClass` field, a `derive_wasm_class` that computed it, a
//! `NativeUdfOnly` variant and a test asserting `PureSql ⟺ non-Udf`. With no
//! `Udf` variant the bad state is unrepresentable rather than derived and
//! checked, so all four go, and `fossil-runtime/src/udf.rs` with them. **The
//! language now runs entirely in the browser**, which is a change of product
//! and not of housekeeping.

use std::sync::LazyLock;

use fossil_graph_schema::Primitive;
use smol_str::SmolStr;

use crate::ty::{Ty, TyKind};

/// The process-wide catalog.
///
/// The checker resolves every call against this, so it is built once rather
/// than per call. It is program-invariant in v0.1 — there is no federation and
/// no user-defined function, so nothing about a program can change it. When
/// that stops being true this becomes a Salsa input, and the call sites do not
/// move.
static STDLIB: LazyLock<FunctionRegistry> = LazyLock::new(FunctionRegistry::stdlib_default);

/// The stdlib catalog. See [`STDLIB`].
#[must_use]
pub fn stdlib() -> &'static FunctionRegistry {
    &STDLIB
}
use std::collections::HashMap;

/// Registry of stdlib functions available to a Fossil program.
///
/// Construct via [`Self::stdlib_default`]. Read a row by its dotted name with
/// [`Self::lookup`], by receiver and member with [`Self::lookup_member`], and
/// enumerate a receiver's members with [`Self::members_of`].
#[derive(Debug, Clone)]
pub struct FunctionRegistry {
    entries: HashMap<SmolStr, RegistryEntry>,
}

/// What sits to the LEFT of the dot, and therefore what decides which members
/// exist (`grammar.bnf`, `PostfixExpr`).
///
/// This is the field that turns dispatch from a string match into a type
/// question. Before it, `where` / `select` / `join` were productions in the
/// grammar and everything else was looked up by its dotted spelling, so a new
/// verb meant a new RULE. With a receiver on every row, **a new verb is a ROW.**
///
/// The three variants are the three things the grammar says can stand on the
/// left, reduced to what the checker can actually dispatch on:
///
/// - [`Self::Namespace`] — `io`, `core`, `parse`, `math`, `validate`, `anon`.
///   A name, not a value: there is nothing to dispatch on, so the entry is
///   reached by its dotted spelling and only that. `io` is the clearest case —
///   its entries **create** the thing, so they cannot have a receiver.
/// - [`Self::Scalar`] — a value's type. Reached by EITHER path, and both reach
///   ONE row: `str.lower(x)` and `x.lower()` resolve to the same entry,
///   exactly as `str::len(&s)` ≡ `s.len()` in Rust. One entry, two ways in.
/// - [`Self::Relation`] — a relation (`Users`, `Adults`). Its members are the
///   verbs — `where`, `select`, `join` — which is why a new verb is a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// No receiver — a namespace function, reached only as `ns.member(args)`.
    Namespace,
    /// A value of this scalar type. Reached as `Ty.member(x, …)` or
    /// `x.member(…)`; both are the same row.
    Scalar(ScalarTy),
    /// A relation. Reached as `Rel.member(…)`; the members are the verbs.
    Relation,
}

/// The receiver a namespace-or-type head denotes, and the single place the
/// classification lives.
///
/// Derived from the head of the dotted name so that a row cannot carry a
/// receiver that disagrees with its own spelling. In the declarative catalogue
/// this becomes a column and this function becomes its parser; until then it is
/// the parser of a column that is written as the head of the name.
///
/// `str` and `seq` are TYPES, not namespaces: the left of a dot names a thing
/// whose members these are.
#[must_use]
pub fn receiver_of(head: &str) -> Receiver {
    match head {
        "str" => Receiver::Scalar(ScalarTy::String),
        "seq" => Receiver::Relation,
        _ => Receiver::Namespace,
    }
}

/// Split a dotted catalogue name into its receiver and its member.
///
/// The ONE place `recv` and `member` come from. Both are derived rather than
/// passed, because a field a call site sets by hand is a field that drifts from
/// the name written beside it — which is the defect this whole change is about,
/// one level down.
///
/// A name with no dot is not a catalogue name; it gets [`Receiver::Namespace`]
/// and itself as the member, so a malformed row is inert rather than a panic.
#[must_use]
pub fn split_receiver(dotted: &str) -> (Receiver, SmolStr) {
    dotted.split_once('.').map_or_else(
        || (Receiver::Namespace, SmolStr::new(dotted)),
        |(head, member)| (receiver_of(head), SmolStr::new(member)),
    )
}

/// A single stdlib function entry — one ROW of the catalogue.
///
/// Four fields, all data: the `Receiver` it hangs off, its name, a `'db`-free
/// [`SigSpec`], and its `LoweringKind`. `recv` and `member` are derived from
/// `name` by `split_receiver` at construction and are never passed in.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// Fully-qualified dotted name (e.g. `"str.trim"`, `"io.csv"`).
    ///
    /// Still the catalogue's key, because a namespace entry has no other handle.
    /// For a [`Receiver::Scalar`] or [`Receiver::Relation`] row it is the
    /// TYPE-PATH spelling: the left half names a type, not a namespace.
    pub name: SmolStr,
    /// What this entry hangs off. See `Receiver`.
    pub recv: Receiver,
    /// The name after the dot — the member. `"lower"` for `str.lower`, `"where"`
    /// for the relation verb. This is what a value-path call `x.lower()` matches
    /// on, and it is why the type path `str.lower(x)` and the value path
    /// `x.lower()` reach one row.
    pub member: SmolStr,
    /// `'db`-free signature description. The checker reads it directly —
    /// `param.ty.to_ty(db)` — and there is no interned form of it.
    pub sig: SigSpec,
    /// How this row compiles: a scalar SQL expression, or an operator.
    pub lowering: LoweringKind,
}

/// A `'db`-free description of a function signature: the param scalar types and
/// the return scalar type.
///
/// Stored in the static registry so a [`RegistryEntry`] needs no `'db` lifetime.
/// A parameter's type is resolved one at a time, where it is checked.
///
/// v0.1 limitation: the surface syntax for schema-directed parsing (`parse.json`
/// and the `seq/` higher-order arguments) does not exist yet, so signatures here
/// are the best scalar approximation. Generic / higher-order positions collapse
/// to their dominant scalar shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigSpec {
    /// Each parameter's NAME and scalar type, in order.
    pub params: Vec<ParamSpec>,
    /// What the row gives back.
    pub ret: SigTy,
}

/// One parameter of a catalogue row: what it is called, and what it takes.
///
/// The name is here and nowhere else, and that placement is the whole of the
/// named-argument decision (`grammar.bnf`, `NamedArg`). `NamedArg := IDENT ASSIGN
/// Expression` is a production, so the grammar is normative and `format =
/// "%Y-%m-%d"` is a program fossil must accept; what it needed was somewhere for
/// `format` to MEAN something. A signature is that somewhere:
///
/// - it is where arity and argument types are already checked, so a name that
///   matches no parameter is refused beside a count that does not match;
/// - it makes the resolution PURELY LOCAL — `crate::lower` reorders the
///   arguments into positional slots against this vector and every layer below
///   the HIR keeps seeing positional arguments. `HirExpr::Call` did not change,
///   MIR did not change, and the backends did not change.
///
/// The rejected alternative was to carry names into `HirExpr::Call` and resolve
/// them in the checker. It costs a wider HIR for nothing: no diagnostic the
/// checker could give is better than one given where the source text is still in
/// hand, and every consumer of `Call` would then have to know that arguments may
/// be out of order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSpec {
    /// What a named argument writes to reach this position.
    pub name: SmolStr,
    /// What it takes.
    pub ty: SigTy,
}

/// What a catalogue row's parameter takes, or what it gives back.
///
/// It was [`ScalarTy`] alone, and the `seq/` rows were written
/// `p("rows", S::String)` under a comment reading «higher-order arguments
/// collapse to scalar placeholders in v0.1». They did not collapse; there was
/// nothing to collapse INTO, because a relation was not a type. It is
/// [`crate::ty::TyKind::Relation`] now, and these two variants are the rest of
/// the notation:
///
/// - [`Self::Rows`] is the relation a verb is a verb OF, and every `seq/` row's
///   parameter 0;
/// - [`Self::Predicate`] is an expression over that relation's row, yielding
///   `Bool` — what `where` takes and what a `join`'s `on` condition is
///   (ruling 17).
///
/// It is the shape five reference systems arrive at from different directions:
/// `pg_proc`'s argument types over a catalogue, GHC's `primops.txt.pp`, Trino's
/// and `DuckDB`'s function tables, and PRQL's `{arg:N}`. What none of them needed
/// and this does is a parameter whose type depends on the RECEIVER's, which is
/// why `Rows` and `Predicate` are variants and not two more scalars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigTy {
    /// A value of a scalar type.
    Scalar(ScalarTy),
    /// The receiver relation, with whatever rows it carries.
    Rows,
    /// A condition over the receiver's rows.
    Predicate,
}

impl SigTy {
    /// The scalar this position takes, or `None` when it takes a relation or a
    /// condition — neither of which a `'static` tag can describe, because both
    /// depend on the receiver.
    #[must_use]
    pub const fn scalar(self) -> Option<ScalarTy> {
        match self {
            Self::Scalar(s) => Some(s),
            Self::Rows | Self::Predicate => None,
        }
    }
}

impl From<ScalarTy> for SigTy {
    fn from(s: ScalarTy) -> Self {
        Self::Scalar(s)
    }
}

impl SigSpec {
    /// Convenience constructor.
    #[must_use]
    pub const fn new(params: Vec<ParamSpec>, ret: SigTy) -> Self {
        Self { params, ret }
    }

    /// The zero-based position `name` refers to, or `None` if this signature has
    /// no such parameter.
    #[must_use]
    pub fn position_of(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }
}

/// A `'db`-free scalar-type tag, mirroring the subset of `fossil_hir::TyKind`
/// the v0.1 stdlib signatures need. Resolved to a `Ty<'db>` by [`Self::to_ty`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarTy {
    /// `xsd:string`.
    String,
    /// `xsd:integer`.
    Integer,
    /// `xsd:float` / `xsd:double`.
    Float,
    /// `xsd:boolean`.
    Bool,
    /// `xsd:date`.
    Date,
    /// `xsd:dateTime`.
    DateTime,
    // An `Iri` variant stood here and no row in the catalogue used it — a
    // signature can neither take nor return a reference, because a reference is
    // to a SHAPE and the catalogue names no shapes. When one does, it will need
    // to say WHICH, which is not a scalar tag.
    /// `Seq<String>` — the one repeated shape v0.1 needs (`str.split`).
    SeqString,
}

impl ScalarTy {
    /// Intern this tag into a `Ty<'db>` via the fossil-hir constructor.
    #[must_use]
    pub fn to_ty(self, db: &dyn salsa::Database) -> Ty<'_> {
        let kind = match self {
            Self::String => TyKind::Primitive(Primitive::String),
            Self::Integer => TyKind::Primitive(Primitive::Integer),
            Self::Float => TyKind::Primitive(Primitive::Float),
            Self::Bool => TyKind::Primitive(Primitive::Bool),
            Self::Date => TyKind::Primitive(Primitive::Date),
            Self::DateTime => TyKind::Primitive(Primitive::DateTime),
            Self::SeqString => {
                let s = Ty::new(db, TyKind::Primitive(Primitive::String));
                TyKind::Seq(s)
            }
        };
        Ty::new(db, kind)
    }
}

/// How a stdlib row compiles. TWO variants, and the border between them is the
/// only one that was ever semantic.
///
/// # `Expr` — a scalar SQL expression, written as a template
///
/// The payload is SQL text with `%N` holes, `N` the ZERO-BASED index of an
/// argument. `trim(%0)`, `CAST(%0 AS BIGINT)`, `%0 || %1`.
///
/// **Why indexed holes and not sequential ones.** A `?`-style placeholder that
/// consumes the next argument cannot express the two shapes the catalogue
/// actually contains, and both are load-bearing:
///
/// - a hole appearing **twice** — `core.require` is
///   `CASE WHEN %0 IS NULL THEN error(…) ELSE %0 END`, one argument read in two
///   positions. Sequential placeholders would demand two arguments for a
///   one-argument function;
/// - a hole appearing **zero** times — `anon.redact` is the literal
///   `'[REDACTED]'` and ignores what it is given. Sequential placeholders
///   cannot say "ignore".
///
/// So a template does NOT determine arity, and must not: [`SigSpec`] does, and
/// the checker reads it. A hole index outside the signature is a catalogue bug
/// that `crate::stdlib::tests` catches, not a runtime one.
///
/// **The one shape this notation does not have is the variadic**, and that is a
/// deliberate hole rather than an oversight. `Concat` was the only variadic
/// form and it belonged to `core.triple`, which was deleted with the other five
/// RDF term constructors; `BlankNode` was the only template referring to
/// something that is not an argument at all (`'_:bnode_' || row_id`) and
/// belonged to `core.blank`, deleted in the same sweep. Two of the four arity
/// puzzles this notation had to answer answered themselves by being
/// unreachable, and the remaining two are the two above. A variadic row needs a
/// notation and there is no row that needs one.
///
/// # `Op` — an operator of the algebra
///
/// [`PlanOp`] cuts at a real seam: it names one of a closed set of 14 operators
/// that affect the PLAN, not a value. It used to be called `Plan`, after its
/// effect, rather than after what it names.
///
/// # What `Udf` cost, and what deleting it bought
///
/// There was a third kind, `Udf`, naming a native Rust function registered on a
/// `DuckDB` connection. Eight rows used it. Two are deleted from the language
/// outright because they cannot be done honestly without Rust — `anon.hmac`
/// (HMAC needs a key schedule and `DuckDB` has `sha256` and no HMAC) and
/// `clean.normalize_unicode` (`DuckDB` ships `nfc_normalize`, which is NFC and
/// nothing else). The other six are templates here, and each one's delta
/// against its Rust predecessor was measured, not assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoweringKind {
    /// A scalar SQL expression template with `%N` argument holes.
    Expr(SmolStr),
    /// An operator of the algebra — affects the plan, not a value.
    Op(PlanOp),
}

/// Operators of the algebra: the relation verbs plus the `io/` sources.
///
/// These mirror the multi-input MIR ops. They were documented as
/// "surface-unreachable in v0.1", and that stopped being true with the
/// receiver: a [`Receiver::Relation`] row is reached by writing
/// `User.where(User.age >= 18)`, which `grammar.bnf, PostfixExpr` spells and the
/// conformance programs use.
///
/// The gap that is now VISIBLE rather than hidden: this enum has 13 relation
/// operators and `crate::lower::lower_source_stage` implements three
/// (`where`, `select`, `join`). Before the receiver, nothing could enumerate
/// the members of a relation and so nothing could count the difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOp {
    /// `seq.where` → `FilterOp` (`WHERE`).
    Where,
    /// `seq.map` → `ExtendOp`s + `ProjectOp`.
    Map,
    /// `seq.flatten` → `UNNEST`.
    Flatten,
    /// `seq.take` → `LIMIT n`.
    Take,
    /// `seq.drop` → `OFFSET n`.
    Drop,
    /// `seq.distinct` → `DISTINCT` / `DISTINCT ON`.
    Distinct,
    /// `seq.sort` → `ORDER BY`.
    Sort,
    /// `seq.select` → `ProjectOp` (`SELECT cols`).
    Select,
    /// `seq.join` → `JoinOp` (`JOIN ... ON`).
    Join,
    /// `seq.union` → `UNION ALL`.
    Union,
    /// `seq.group_by` → marks input for `GROUP BY`.
    GroupBy,
    /// `seq.aggregate` → `SELECT keys, aggs ... GROUP BY keys`.
    Aggregate,
    /// `seq.count` → `COUNT(*)`.
    Count,
    /// An `io/` source constructor. **Which** reader it is is not written here.
    ///
    /// It was `Source(SourceFormatTag)`, a registry-local three-variant mirror
    /// of `fossil-mir::SourceFormat`, and it was write-only: constructed at the
    /// three `io.` rows below and destructured nowhere — every reader of
    /// [`LoweringKind`] matches `Expr` and takes `Op(_)` as a wildcard
    /// (`fossil_df::lower_call`, `fossil_df::stdlib`). It was the reader half of
    /// `fossil_base::providers` said a second time, which is the defect ruling
    /// 13 collapsed one level up: `fossil-mir::lower::resolve_source` resolves
    /// the format from the PROVIDER ROW (`Provider::reads_rows`) and has never
    /// read this.
    ///
    /// So the row says what a `HirExpr::Call` needs it to say — *this name is a
    /// source, not a scalar expression* — and the catalogue that owns readers
    /// answers which one.
    Source,
}

// ── Source dispatch lived here, and it was HALF of one table ──────────────
//
// `SourceKind` / `SOURCE_KINDS` / `source_kind` are gone to
// `fossil_base::providers`. They described a name, the extensions it accepts,
// and what it does with them — the same three fields as `ShapeDecoder` in
// `fossil-base`, modelled twice: this one dispatched by NAME, that one by
// EXTENSION, and `def_map` threw away the constructor of a `type { … } :=`
// binding because nothing read it. `io.shex("x.ttl")` and `io.shacl("x.ttl")`
// were therefore the same program. Ruling 13 of `SURFACE-PLAN.md` collapses
// them into `fossil_base::providers::Provider`, one lookup and one criterion.
//
// The table cannot live here any more even if it wanted to: a row that reads
// TYPES carries a `fn` into a schema language, and `fossil-hir` may not link
// one (`0e6898d`). It comes from the host, through `System::providers`.
//
// `FunctionRegistry` below is NOT that table and does not merge into it: it
// carries call SIGNATURES for the checker, and `io.csv` appears in both for the
// same reason `str.trim` appears in one — a provider is a thing you can name,
// a signature is what happens when you call it.

impl FunctionRegistry {
    /// Construct the stdlib catalog: every function across the seven surface
    /// namespaces-and-types (`seq`/`core`/`parse`/`math`/`str`/`validate`/
    /// `anon`) plus the `io/` source constructors.
    ///
    /// `clean/` is not among them any more, and that is ruling 16 of
    /// `SURFACE-PLAN.md`: it held `trim`, `lower`, `upper`, `slug` and
    /// `strip_html` while `str/` held eight operations on a string, with no
    /// principle separating the two — `replace` could have been called cleaning
    /// and `trim` could have been called a string operation. Five renames and
    /// the namespace is gone. It also makes `str.lower(str.trim(x))` — the
    /// canonical example of one entry reached two ways, and the one
    /// `grammar.bnf, PostfixExpr` gives — name two rows that exist, which it did
    /// not before.
    #[must_use]
    #[allow(clippy::too_many_lines)] // a flat catalog table; one row per stdlib fn.
    pub fn stdlib_default() -> Self {
        use PlanOp as P;
        use ScalarTy as S;

        let mut reg = Self {
            entries: HashMap::new(),
        };

        // One parameter. The name is what a named argument writes to reach this
        // position (`grammar.bnf`, NamedArg) — see [`ParamSpec`] — and the
        // FIRST parameter of a `Receiver::Scalar` or `Receiver::Relation` row is
        // the receiver itself, which `x.trim()` fills by being written to the
        // left of the dot. It is named anyway: `str.trim(text = x)` is the type
        // path, and refusing a name in the one position that has two spellings
        // would be a rule with no reason.
        let p = |name: &str, ty: ScalarTy| ParamSpec {
            name: SmolStr::new(name),
            ty: SigTy::Scalar(ty),
        };
        // The relation a verb is a verb of, and a condition over its rows —
        // see [`SigTy`]. Every `seq/` row's parameter 0 is `rows`.
        let rows = |name: &str| ParamSpec {
            name: SmolStr::new(name),
            ty: SigTy::Rows,
        };
        let pred = |name: &str| ParamSpec {
            name: SmolStr::new(name),
            ty: SigTy::Predicate,
        };

        // Local insertion helper. `recv` and `member` come from
        // `split_receiver` and from nowhere else, so the row's receiver cannot
        // disagree with the row's name.
        let add = |entries: &mut HashMap<SmolStr, RegistryEntry>,
                   name: &str,
                   params: Vec<ParamSpec>,
                   ret: ScalarTy,
                   lowering: LoweringKind| {
            let (recv, member) = split_receiver(name);
            entries.insert(
                SmolStr::new(name),
                RegistryEntry {
                    name: SmolStr::new(name),
                    recv,
                    member,
                    sig: SigSpec::new(params, SigTy::Scalar(ret)),
                    lowering,
                },
            );
        };
        // A verb of the algebra: it takes a relation and gives one back. A
        // separate helper from `add` and not a flag on it, because the two say
        // different things — the split `LoweringKind` already draws between a
        // scalar expression and a `PlanOp`.
        let add_verb = |entries: &mut HashMap<SmolStr, RegistryEntry>,
                        name: &str,
                        params: Vec<ParamSpec>,
                        lowering: LoweringKind| {
            let (recv, member) = split_receiver(name);
            entries.insert(
                SmolStr::new(name),
                RegistryEntry {
                    name: SmolStr::new(name),
                    recv,
                    member,
                    sig: SigSpec::new(params, SigTy::Rows),
                    lowering,
                },
            );
        };
        let e = &mut reg.entries;

        // ── core/ (2) ─────────────────────────────────────────────────────
        //
        // This used to be eight, and the six that went were RDF term
        // constructors — `iri`, `triple`, `blank`, `literal`, `typed`, `emit`.
        // Not one `.fossil` program in the tree called any of them. They took
        // `InlineForm::Concat` and `InlineForm::BlankNode` with them, which is
        // why the template notation above needs neither a variadic nor a hole
        // that is not an argument.
        add(
            e,
            "core.lang",
            vec![p("value", S::String), p("tag", S::String)],
            S::String,
            expr("%0"),
        );
        // require: forall T. T? -> T. v0.1 scalar approximation String -> String.
        // The one row whose template reads its argument TWICE.
        add(
            e,
            "core.require",
            vec![p("value", S::String)],
            S::String,
            expr("CASE WHEN %0 IS NULL THEN error('core.require: value is null') ELSE %0 END"),
        );

        // ── seq/ (13) — the relation verbs. Receiver::Relation ─────────────
        //
        // Higher-order arguments collapse to scalar placeholders in v0.1; the
        // `PlanOp` tag is the real datum. `filter` and `project` were renamed to
        // `where` and `select` so that the catalogue spells what the surface
        // spells — `members_of(Relation)` is what an IDE offers, and offering
        // `User.filter(…)` for a language whose word is `where` is a completion
        // that is confidently wrong.
        // The three the surface reaches — `where`, `select`, `join` — carry the
        // signature they always had and could not write down. The other ten are
        // rows with no lowering in `crate::lower::lower_source_stage`, and they
        // say so by their `PlanOp`; what they gain here is that the day one is
        // implemented, its arguments are checked by the same code that checks
        // `str.trim`'s.
        add_verb(
            e,
            "seq.where",
            vec![rows("rows"), pred("keep")],
            L(P::Where),
        );
        add_verb(e, "seq.map", vec![rows("rows")], L(P::Map));
        add_verb(e, "seq.flatten", vec![rows("rows")], L(P::Flatten));
        add_verb(
            e,
            "seq.take",
            vec![rows("rows"), p("n", S::Integer)],
            L(P::Take),
        );
        add_verb(
            e,
            "seq.drop",
            vec![rows("rows"), p("n", S::Integer)],
            L(P::Drop),
        );
        add_verb(e, "seq.distinct", vec![rows("rows")], L(P::Distinct));
        add_verb(e, "seq.sort", vec![rows("rows")], L(P::Sort));
        // `select`'s columns are NAMES and not values, so they are not
        // parameters: `crate::lower` reads them off the CST as
        // `SelectedColumn`s. The signature says what the verb takes of the
        // ALGEBRA, which is the relation.
        add_verb(e, "seq.select", vec![rows("rows")], L(P::Select));
        // A join's condition is a predicate over BOTH sides (ruling 17), which
        // is the relation this row hands the checker once the right side is in.
        add_verb(
            e,
            "seq.join",
            vec![rows("rows"), rows("other"), pred("on")],
            L(P::Join),
        );
        add_verb(
            e,
            "seq.union",
            vec![rows("rows"), rows("other")],
            L(P::Union),
        );
        add_verb(e, "seq.group_by", vec![rows("rows")], L(P::GroupBy));
        add_verb(e, "seq.aggregate", vec![rows("rows")], L(P::Aggregate));
        // The one `seq/` row that is not a verb of the algebra: it takes a
        // relation and gives back a NUMBER.
        add(e, "seq.count", vec![rows("rows")], S::Integer, L(P::Count));

        // ── str/ (13) — every operation on a string. Receiver::Scalar(String)
        //
        // Eight were here and five arrived from `clean/` (ruling 16):
        // `trim`, `lower`, `upper`, `slug`, `strip_html`.
        // `clean.normalize_unicode` did NOT arrive: it is deleted from the
        // language, because `DuckDB` has `nfc_normalize` and therefore NFC
        // only, and a `normalize_unicode(x, 'NFKD')` that silently gave NFC
        // would be a function that lies.
        add(
            e,
            "str.length",
            vec![p("text", S::String)],
            S::Integer,
            expr("length(%0)"),
        );
        add(
            e,
            "str.slice",
            vec![p("text", S::String), p("start", S::Integer)],
            S::String,
            expr("substring(%0, %1)"),
        );
        add(
            e,
            "str.contains",
            vec![p("text", S::String), p("needle", S::String)],
            S::Bool,
            expr("contains(%0, %1)"),
        );
        add(
            e,
            "str.starts_with",
            vec![p("text", S::String), p("prefix", S::String)],
            S::Bool,
            expr("starts_with(%0, %1)"),
        );
        add(
            e,
            "str.ends_with",
            vec![p("text", S::String), p("suffix", S::String)],
            S::Bool,
            expr("ends_with(%0, %1)"),
        );
        add(
            e,
            "str.replace",
            vec![
                p("text", S::String),
                p("needle", S::String),
                p("replacement", S::String),
            ],
            S::String,
            expr("replace(%0, %1, %2)"),
        );
        add(
            e,
            "str.split",
            vec![p("text", S::String), p("separator", S::String)],
            S::SeqString,
            expr("string_split(%0, %1)"),
        );
        add(
            e,
            "str.concat",
            vec![p("text", S::String), p("other", S::String)],
            S::String,
            expr("concat(%0, %1)"),
        );
        add(
            e,
            "str.trim",
            vec![p("text", S::String)],
            S::String,
            expr("trim(%0)"),
        );
        add(
            e,
            "str.lower",
            vec![p("text", S::String)],
            S::String,
            expr("lower(%0)"),
        );
        add(
            e,
            "str.upper",
            vec![p("text", S::String)],
            S::String,
            expr("upper(%0)"),
        );
        add(
            e,
            "str.slug",
            vec![p("text", S::String)],
            S::String,
            expr(SLUG_TEMPLATE),
        );
        add(
            e,
            "str.strip_html",
            vec![p("text", S::String)],
            S::String,
            expr(STRIP_HTML_TEMPLATE),
        );

        // ── parse/ (7) — casts + strptime + json + csv_row ─────────────────
        add(
            e,
            "parse.integer",
            vec![p("text", S::String)],
            S::Integer,
            expr("CAST(%0 AS BIGINT)"),
        );
        add(
            e,
            "parse.float",
            vec![p("text", S::String)],
            S::Float,
            expr("CAST(%0 AS DOUBLE)"),
        );
        // decimal: no Decimal type in MVP → returns Float.
        add(
            e,
            "parse.decimal",
            vec![p("text", S::String)],
            S::Float,
            expr("CAST(%0 AS DECIMAL(38,18))"),
        );
        add(
            e,
            "parse.date",
            vec![p("text", S::String), p("format", S::String)],
            S::Date,
            expr("strptime(%0, %1)"),
        );
        add(
            e,
            "parse.datetime",
            vec![p("text", S::String), p("format", S::String)],
            S::DateTime,
            expr("strptime(%0, %1)"),
        );
        // json: forall T. (String, path) -> T. Schema-directed parsing is
        // post-surface-syntax; v0.1 lowers to a scalar extract. The old
        // signature declared ONE parameter while its own doc-comment wrote
        // `json_extract(s, path)` — the second argument is now in the signature
        // instead of only in the prose.
        add(
            e,
            "parse.json",
            vec![p("text", S::String), p("path", S::String)],
            S::String,
            expr("json_extract(%0, %1)"),
        );
        // csv_row had the same defect and worse: two declared parameters, a
        // doc-comment reading `split_part(s, sep, n)`, and no renderer, so
        // nothing ever noticed. The field index is an argument now.
        add(
            e,
            "parse.csv_row",
            vec![
                p("text", S::String),
                p("separator", S::String),
                p("field", S::Integer),
            ],
            S::String,
            expr("split_part(%0, %1, %2)"),
        );

        // ── math/ (6 — EXACTLY; NO ceil/floor) ─────────────────────────────
        add(
            e,
            "math.sum",
            vec![p("value", S::Float)],
            S::Float,
            expr("sum(%0)"),
        );
        add(
            e,
            "math.avg",
            vec![p("value", S::Float)],
            S::Float,
            expr("avg(%0)"),
        );
        add(
            e,
            "math.min",
            vec![p("value", S::Float)],
            S::Float,
            expr("min(%0)"),
        );
        add(
            e,
            "math.max",
            vec![p("value", S::Float)],
            S::Float,
            expr("max(%0)"),
        );
        add(
            e,
            "math.abs",
            vec![p("value", S::Float)],
            S::Float,
            expr("abs(%0)"),
        );
        add(
            e,
            "math.round",
            vec![p("value", S::Float)],
            S::Integer,
            expr("round(%0)"),
        );

        // ── validate/ (5) ──────────────────────────────────────────────────
        //
        // Four were native Rust UDFs that returned the input when valid and
        // raised a `DuckDB` error when not. The shape survives — the value or
        // an error, never a null — and the PREDICATE is now SQL. Each delta
        // against the Rust it replaces was measured against a real `DuckDB`.
        add(
            e,
            "validate.email",
            vec![p("value", S::String)],
            S::String,
            expr(VALIDATE_EMAIL_TEMPLATE),
        );
        add(
            e,
            "validate.url",
            vec![p("value", S::String)],
            S::String,
            expr(VALIDATE_URL_TEMPLATE),
        );
        add(
            e,
            "validate.uuid",
            vec![p("value", S::String)],
            S::String,
            expr(VALIDATE_UUID_TEMPLATE),
        );
        add(
            e,
            "validate.iso_date",
            vec![p("value", S::String)],
            S::String,
            expr(VALIDATE_ISO_DATE_TEMPLATE),
        );
        add(
            e,
            "validate.regex",
            vec![p("value", S::String), p("pattern", S::String)],
            S::String,
            expr("regexp_matches(%0, %1)"),
        );

        // ── anon/ (2) ──────────────────────────────────────────────────────
        //
        // `anon.hmac` was here and is deleted from the language. HMAC needs a
        // key schedule; `DuckDB` has `sha256` and no HMAC, so the only honest
        // renderings were a native UDF (gone with `Udf`) or a thing called
        // `hmac` that is not one.
        // `salt` is a parameter and not a convenience. A hash of an email with
        // no salt is a rainbow-table lookup, so a row called `anon.hash` that
        // takes only the value does not anonymise — and the docs already
        // described the salted form («`salt` and `format` are named because the
        // call would be a puzzle otherwise»), which is the rule read
        // literally: the design was right and the row was short.
        //
        // No native level needed. Concatenation is a template, so the whole
        // thing lowers to one expression, and the `vscalar` door
        // `Cargo.toml` still holds open stays shut for this one.
        add(
            e,
            "anon.hash",
            vec![p("value", S::String), p("salt", S::String)],
            S::String,
            expr("sha256(%0 || %1)"),
        );
        add(
            e,
            "anon.redact",
            vec![p("value", S::String)],
            S::String,
            // The one row whose template reads NO argument.
            expr("'[REDACTED]'"),
        );

        // ── io/ (3 in v0.1) — source constructors ──────────────────────────
        add(
            e,
            "io.csv",
            vec![p("uri", S::String)],
            S::String,
            L(P::Source),
        );
        add(
            e,
            "io.json",
            vec![p("uri", S::String)],
            S::String,
            L(P::Source),
        );
        add(
            e,
            "io.parquet",
            vec![p("uri", S::String)],
            S::String,
            L(P::Source),
        );

        reg
    }

    /// Lookup a row by fully-qualified dotted name — the TYPE path,
    /// `str.trim(x)`. Returns `None` if the name is unknown.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.get(name)
    }

    /// Lookup a row by RECEIVER and MEMBER — the VALUE path, `x.trim()`.
    ///
    /// This and [`Self::lookup`] reach the same row for a
    /// [`Receiver::Scalar`] entry: one entry, two ways in, and no second row
    /// to keep in step.
    #[must_use]
    pub fn lookup_member(&self, recv: Receiver, member: &str) -> Option<&RegistryEntry> {
        self.entries
            .values()
            .find(|e| e.recv == recv && e.member == member)
    }

    /// Every member of a receiver, unordered.
    ///
    /// The query that makes completion stop being approximate: an IDE asks
    /// what a value HAS instead of offering the catalogue and hoping.
    pub fn members_of(&self, recv: Receiver) -> impl Iterator<Item = &RegistryEntry> {
        self.entries.values().filter(move |e| e.recv == recv)
    }

    /// Every row whose member is spelled `member`, across all receivers.
    ///
    /// The value path knows the member before it knows the receiver's type, so
    /// this is what a lowering can ask. Exactly one hit is a resolution; more
    /// than one needs the type and is the checker's to settle.
    pub fn candidates_for_member(&self, member: &str) -> impl Iterator<Item = &RegistryEntry> {
        self.entries
            .values()
            .filter(move |e| e.member == member && e.recv != Receiver::Namespace)
    }

    /// Is `head` the left half of any catalogued name?
    ///
    /// What separates `str.slug` from `orders.user_id` used to be an
    /// open-coded `starts_with(&format!("{head}."))` scan in the lowering. It is
    /// a question about the catalogue, so it lives here.
    #[must_use]
    pub fn is_catalogued_head(&self, head: &str) -> bool {
        let prefix = format!("{head}.");
        self.entries.keys().any(|k| k.starts_with(&prefix))
    }

    /// Iterate every registered entry (unspecified order).
    pub fn iter(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.entries.values()
    }
}

// ── The measured templates ─────────────────────────────────────────────────
//
// The six that were native Rust UDFs. Each one's text was measured against
// `DuckDB` before it was written here, and each carries the delta it has
// against the Rust it replaces — because "equivalent" was the claim that had to
// be checked, not the claim that could be assumed.

// Two facts about `CASE … error(…) END` that the four validators below rest on,
// both MEASURED against DuckDB v1.5.3 rather than assumed:
//
// 1. `error()` in a `CASE` arm is LAZY, and lazy PER ROW — a column of 5,000
//    valid values returns 5,000 values, and the one bad row at index 4,999 is
//    what raises. Had it been eager, every one of these would be a query that
//    always fails.
// 2. A NULL predicate takes the ELSE branch, so the naive shape RAISES ON NULL.
//    `regexp_matches(NULL, …)` is NULL, not false. Every validator therefore
//    opens `%0 IS NULL OR …`, which passes NULL through unchanged — the
//    behaviour a UDF over a nullable column had.
//
// And one difference from a UDF that no template can close, recorded because it
// is real: DuckDB may prune a projection nobody consumes, so a validator whose
// value is never read does not run. `SELECT count(*)` over a column with a bad
// row measured 5,000, not an error. Validation happens where the value is USED.

/// `str.slug` — replaces the `fossil_slug` UDF.
///
/// **The template this was specified with matched NEITHER Rust implementation,
/// and measurement is the only reason we know.** The tree once carried two slug
/// functions: the `fossil_slug` UDF, which was `slug::slugify`, and a
/// hand-written one that kept `.`, lower-cased, and collapsed every run of
/// non-`\p{L}\p{N}.-` to a single `-`. Against a 16-case hand corpus the
/// specified `'[^a-z0-9.-]+'` form scored 8/16 against the first and 7/16
/// against the second — it is a hybrid of the two, keeping `.` like the one and
/// collapsing runs like the other. Worse, it maps `ÅÄÖ` and `日本語` to the
/// EMPTY STRING, which in a slug that ends up in an IRI is an identity
/// collision and not a formatting difference.
///
/// This form is EXACT against that hand-written one: 16/16 by hand and 0
/// mismatches over 240,998 fuzzed inputs. `slug::slugify` cannot be reproduced at all —
/// it TRANSLITERATES (`Straße` → `strasse`, `€100` → `eur100`, `Москва` →
/// `moskva`) and `DuckDB` ships no transliteration function. The best available
/// approximation, over `strip_accents`, differs on 55.6% of Latin-accented
/// input, so this keeps the implementation whose behaviour is expressible and
/// says so.
const SLUG_TEMPLATE: &str =
    r"trim(regexp_replace(lower(trim(%0)), '[^\p{L}\p{N}.-]', '-', 'g'), '-')";

/// `str.strip_html` — replaces `voca_rs::strip::strip_tags`.
///
/// **NOT exact, and this is the one template that is not**: 27/29 by hand and
/// 1,837 mismatches over 120,000 fuzzed inputs (1.5%). `voca_rs` is a parser
/// and this is a regex, so the residue is entirely MALFORMED markup — an
/// unbalanced quote makes voca swallow the rest of the string and a regex
/// cannot. With a quote character present the disagreement rate is 10.9%;
/// without one it is 0.6%.
///
/// The naive `'<[^>]*>'` scored 17/29. What this adds is the two things voca
/// actually does: a bare `<` followed by whitespace is TEXT (`a < b` survives),
/// and a quoted attribute may contain `>` (`<a href="x>y">link</a>` → `link`).
const STRIP_HTML_TEMPLATE: &str =
    r#"regexp_replace(%0, '<(?:>|$|[^\s](?:"[^"]*"|''[^'']*''|[^>"''])*(?:>|$))', '', 'g')"#;

/// `validate.uuid` — replaces `uuid::Uuid::parse_str`. **Exact**: 20/20 by hand,
/// 0 mismatches over 120,000 fuzzed inputs.
///
/// The four accepted forms were established by COMPILING the crate, not by
/// reading its documentation: hyphenated, bare 32-hex, braced-and-hyphenated,
/// and `urn:uuid:` — the last lowercase only, and neither the braced nor the
/// urn form accepting a bare 32-hex body.
///
/// `TRY_CAST(%0 AS UUID)` looks like the obvious answer and is wrong in BOTH
/// directions (17/20, 1,058 fuzz mismatches): it rejects `urn:uuid:…` and it
/// accepts hyphens anywhere at all, including
/// `--------550e8400e29b41d4a716446655440000`.
const VALIDATE_UUID_TEMPLATE: &str = "CASE WHEN %0 IS NULL OR regexp_matches(%0, \
     '^(?:urn:uuid:)?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$\
     |^[0-9a-fA-F]{32}$\
     |^\\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\\}$') \
     THEN %0 ELSE error('validate.uuid: not a UUID: ' || %0) END";

/// `validate.iso_date` — replaces the hand-written `is_valid_iso_date`.
/// **Exact**: 24/24 by hand, 0 mismatches over 120,000 fuzzed inputs.
///
/// Including the quirk, which is the part worth naming: the Rust range-checked
/// month and day INDEPENDENTLY and never consulted a calendar, so `2026-02-31`
/// and `2023-02-29` are valid. The regex reproduces that exactly.
///
/// `TRY_CAST(%0 AS DATE)` is not merely more lenient, it SILENTLY TRUNCATES:
/// `2026-05-2a` casts to `2026-05-02` and `26-05-21` to `0026-05-21`. A
/// validator that turns bad input into a different valid value is worse than
/// one that is wrong.
const VALIDATE_ISO_DATE_TEMPLATE: &str = "CASE WHEN %0 IS NULL OR regexp_matches(%0, \
     '^[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])$') \
     THEN %0 ELSE error('validate.iso_date: not a YYYY-MM-DD date: ' || %0) END";

/// `validate.email` — replaces the hand-written `is_valid_email`. **Exact**:
/// 35/35 by hand, 0 mismatches over 120,000 fuzzed inputs (9,972 of them
/// positives). Both delta columns came back EMPTY.
///
/// It agrees on the awkward cases in both directions, which is what makes the
/// zero meaningful: `a b@c.com` and `"quoted local"@b.com` are VALID on both
/// sides (the Rust never looked at the local part), `a@b_c.com` and `a@b..com`
/// are invalid on both.
const VALIDATE_EMAIL_TEMPLATE: &str = "CASE WHEN %0 IS NULL OR regexp_matches(%0, \
     '^[^@]+@[A-Za-z0-9-]+(\\.[A-Za-z0-9-]+)+$') \
     THEN %0 ELSE error('validate.email: not an email: ' || %0) END";

/// `validate.url` — replaces the hand-written `is_valid_url`. **Exact**: 39/39
/// by hand, 0 mismatches over 120,000 fuzzed inputs. Both delta columns EMPTY.
///
/// `(?s)` is load-bearing and was measured, not guessed: without it
/// `https://\n` is false here and true in the Rust, because `DuckDB`'s `$` is
/// RE2's end-of-TEXT and `.` does not cross a newline.
const VALIDATE_URL_TEMPLATE: &str = "CASE WHEN %0 IS NULL OR regexp_matches(%0, \
     '^[A-Za-z][A-Za-z0-9+.-]*://(?s).+$') \
     THEN %0 ELSE error('validate.url: not a URL: ' || %0) END";

/// Helper: a scalar SQL expression template.
fn expr(template: &str) -> LoweringKind {
    LoweringKind::Expr(SmolStr::new(template))
}

/// Helper: an operator of the algebra. Named for brevity in the catalogue table
/// — thirteen `LoweringKind::Op(PlanOp::…)` in a column is a wall of noise.
#[allow(non_snake_case)]
const fn L(op: PlanOp) -> LoweringKind {
    LoweringKind::Op(op)
}

/// Substitute a template's `%N` holes with the caller's rendered arguments.
///
/// The single renderer, so that a template means one thing everywhere. `%N` is
/// zero-based; `%%` is a literal `%`. A hole whose index is out of range is
/// left verbatim rather than dropped — a template that names an argument the
/// signature does not have is a catalogue bug, and a visible one beats SQL that
/// silently loses a term.
///
/// # Errors
///
/// Never. A malformed template renders to text that will fail to parse, which
/// is where it should be caught.
#[must_use]
pub fn render_template(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != '%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i + 1] == '%' {
            out.push('%');
            i += 2;
            continue;
        }
        let mut j = i + 1;
        let mut digits = String::new();
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            digits.push(bytes[j]);
            j += 1;
        }
        if let Some(a) = digits.parse::<usize>().ok().and_then(|n| args.get(n)) {
            out.push_str(a);
            i = j;
        } else {
            out.push('%');
            i += 1;
        }
    }
    out
}

/// The `%N` hole indices a template names, ascending and deduplicated.
///
/// The catalogue's own guard reads this: a template may use fewer holes than
/// the signature has parameters (`anon.redact` uses none) and may repeat one
/// (`core.require` uses `%0` twice), but it may never name an index the
/// signature does not have.
#[must_use]
pub fn template_holes(template: &str) -> Vec<usize> {
    let chars: Vec<char> = template.chars().collect();
    let mut holes = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            i += 1;
            continue;
        }
        if i + 1 < chars.len() && chars[i + 1] == '%' {
            i += 2;
            continue;
        }
        let mut j = i + 1;
        let mut digits = String::new();
        while j < chars.len() && chars[j].is_ascii_digit() {
            digits.push(chars[j]);
            j += 1;
        }
        if let Ok(n) = digits.parse::<usize>() {
            holes.push(n);
            i = j;
        } else {
            i += 1;
        }
    }
    holes.sort_unstable();
    holes.dedup();
    holes
}

#[cfg(test)]
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
