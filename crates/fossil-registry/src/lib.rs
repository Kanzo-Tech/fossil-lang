//! `fossil-registry` — the Fossil v0.1 standard-library function registry.
//!
//! # Phase 5 (STDL-01..07): the classification framework
//!
//! Every stdlib function from `stdlib.md` is recorded as a [`RegistryEntry`]
//! carrying:
//! - its fully-qualified dotted [`RegistryEntry::name`] (`"clean.trim"`),
//! - a Salsa-free [`SigSpec`] from which an interned `fossil_hir::FnSig<'db>`
//!   is materialized on demand,
//! - a [`LoweringKind`] (how it compiles: `Builtin` / `Inline` / `Udf` / `Plan`),
//! - a [`WasmClass`] (`PureSql` = runs in the browser playground;
//!   `NativeUdfOnly` = requires a native Rust UDF, disabled in `DuckDB`-WASM).
//!
//! ## The `PureSql` ⟺ non-Udf invariant (SC#1 / STDL-07)
//!
//! `wasm_class == NativeUdfOnly` **iff** `lowering` is [`LoweringKind::Udf`].
//! Everything `Builtin` / `Inline` / `Plan` is [`WasmClass::PureSql`]. The
//! invariant is mechanically enforced: every catalog insertion sets
//! `wasm_class` via the private [`derive_wasm_class`] helper, so the two fields
//! cannot drift. The unit test suite asserts it holds for every entry, and the
//! SC#1 CI allowlist gate (plan 05-07) reuses the same derivation.
//!
//! ## Why a `&'static` registry (no `Box<dyn>` in Salsa)
//!
//! The registry is read by `fossil-codegen`'s `render_expr` *inside* a
//! `#[salsa::tracked]` query. Per ADR-0003/ADR-0006 and the CLAUDE.md hard
//! rule, no `Box<dyn Trait>` may cross a Salsa boundary. The stdlib is
//! program-invariant in v0.1 (no federation — REG-01 deferred), so the catalog
//! is a plain owned `FunctionRegistry` built once via [`FunctionRegistry::stdlib_default`]
//! and held behind a `LazyLock`/`OnceLock` static at the consumer. Enum
//! dispatch over [`LoweringKind`] replaces trait objects entirely.
//!
//! `FnSig<'db>` itself is `#[salsa::interned]` (it carries a `'db` lifetime and
//! needs a database to construct), so it cannot live in a `'static` value.
//! Instead the static registry stores a [`SigSpec`] (a `'db`-free description of
//! arity + scalar param/return types) and [`RegistryEntry::signature`] interns
//! the real `FnSig<'db>` on demand against the caller's `db`. This satisfies
//! both "construct `FnSig` directly via the fossil-hir constructor" and
//! "the registry is `&'static`".
//!
//! ## Catalog authority
//!
//! The catalog is reconciled to `stdlib.md` as the authoritative spec
//! (CLAUDE.md). The eight surface namespaces — `seq` / `core` / `clean` /
//! `parse` / `math` / `str` / `validate` / `anon` — are populated exactly: the
//! [`crate::tests`] completeness test asserts SET EQUALITY (catalog set ==
//! stdlib.md set), failing on both omissions and extras. Notable per-namespace
//! reconciliations: `math/` has exactly six functions (`sum`, `avg`, `min`,
//! `max`, `abs`, `round` — NO `ceil`/`floor`); `anon/` includes `redact`
//! (an inline literal mask, `PureSql`); `validate/` includes `regex`
//! (`DuckDB` `regexp_matches`, `PureSql`).
//!
//! The `io/` source constructors (`io.csv` / `io.json` / `io.parquet`) are also
//! registered (Plan-kind sources) so the Phase-1 walking-skeleton and the
//! 05-04 source-lowering work keep a single source of truth. They are NOT part
//! of the eight-namespace surface-function completeness set (`io/sql` and
//! `io/http` are out of scope this milestone). See ADR-0015.

use fossil_hir::FnSig;
use fossil_hir::ty::{Primitive, Ty, TyKind};
use smol_str::SmolStr;
use std::collections::HashMap;

/// Registry of stdlib functions available to a Fossil program.
///
/// Construct via [`Self::stdlib_default`] (the full Phase-5 catalog). The
/// Phase-1 [`Self::phase1_default`] entry point is retained as a thin alias.
/// Read entries by name with [`Self::lookup`] or enumerate every entry with
/// [`Self::iter`] (used by 05-03's UDF manifest and 05-07's SC#1 CI gate).
#[derive(Debug, Clone)]
pub struct FunctionRegistry {
    entries: HashMap<SmolStr, RegistryEntry>,
}

/// A single stdlib function entry.
///
/// Carries the fully-qualified name, a `'db`-free [`SigSpec`], its
/// [`LoweringKind`], and the derived [`WasmClass`]. The `wasm_class` field is
/// always set via [`derive_wasm_class`] at construction so the `PureSql` ⟺
/// non-Udf invariant cannot drift.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// Fully-qualified dotted name (e.g. `"clean.trim"`, `"io.csv"`).
    pub name: SmolStr,
    /// `'db`-free signature description. Materialize the real interned
    /// `FnSig<'db>` with [`Self::signature`].
    pub sig: SigSpec,
    /// How this function compiles to SQL / MIR.
    pub lowering: LoweringKind,
    /// SC#1 / STDL-07 tag: pure `DuckDB` SQL vs native-only Rust UDF.
    pub wasm_class: WasmClass,
}

impl RegistryEntry {
    /// Materialize the interned `fossil_hir::FnSig<'db>` for this entry against
    /// the caller's database.
    ///
    /// This is the bridge between the `'static`-friendly [`SigSpec`] held in the
    /// catalog and the `#[salsa::interned]` `FnSig<'db>` the type checker and
    /// codegen consume. It constructs each [`Ty<'db>`] directly via the
    /// fossil-hir constructor (no string-signature parser).
    #[must_use]
    pub fn signature<'db>(&self, db: &'db dyn salsa::Database) -> FnSig<'db> {
        self.sig.to_fn_sig(db)
    }
}

/// A `'db`-free description of a function signature: the param scalar types and
/// the return scalar type.
///
/// Stored in the static registry so a [`RegistryEntry`] needs no `'db` lifetime.
/// [`Self::to_fn_sig`] interns it into a real `FnSig<'db>` on demand.
///
/// v0.1 limitation (RESEARCH Open Q4): the surface syntax for pipelines and
/// schema-directed parsing (`parse.json`, the `seq/` `Fn(...)` arguments,
/// `forall`-polymorphism) does not exist yet, so signatures here are the best
/// scalar approximation. Generic / higher-order positions collapse to their
/// dominant scalar shape (e.g. predicate-taking `seq/` ops are typed
/// `(String) -> String` placeholders; `parse.json` is `(String) -> String`).
/// These are documented per-entry where they deviate from `stdlib.md`'s ideal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigSpec {
    /// Scalar type of each parameter, in order.
    pub params: Vec<ScalarTy>,
    /// Scalar return type.
    pub ret: ScalarTy,
}

impl SigSpec {
    /// Convenience constructor.
    #[must_use]
    pub const fn new(params: Vec<ScalarTy>, ret: ScalarTy) -> Self {
        Self { params, ret }
    }

    /// Intern this spec into a real `fossil_hir::FnSig<'db>`.
    #[must_use]
    pub fn to_fn_sig<'db>(&self, db: &'db dyn salsa::Database) -> FnSig<'db> {
        let params: Vec<Ty<'db>> = self.params.iter().map(|p| p.to_ty(db)).collect();
        let ret = self.ret.to_ty(db);
        FnSig::new(db, params, ret)
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
    /// An IRI value.
    Iri,
    /// An RDF 1.2 triple-as-term.
    TripleTerm,
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
            Self::Iri => TyKind::Iri,
            Self::TripleTerm => TyKind::TripleTerm,
            Self::SeqString => {
                let s = Ty::new(db, TyKind::Primitive(Primitive::String));
                TyKind::Seq(s)
            }
        };
        Ty::new(db, kind)
    }
}

/// How a stdlib function compiles. Enum dispatch — never a `Box<dyn>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoweringKind {
    /// A `DuckDB` scalar/aggregate builtin called by name: `trim`, `lower`,
    /// `strptime`, `sha256`, `regexp_matches`. ⇒ [`WasmClass::PureSql`].
    Builtin {
        /// The `DuckDB` builtin function name (must be non-empty; the SC#1 gate
        /// in 05-07 additionally checks membership in a curated allowlist).
        duckdb_name: SmolStr,
    },
    /// Compiled inline to a SQL expression — a `CAST`, `||`, `CASE`, or literal,
    /// not a named function call. ⇒ [`WasmClass::PureSql`].
    Inline(InlineForm),
    /// A native Rust UDF registered on the `DuckDB` connection (`fossil_slug`,
    /// `fossil_hmac`). NOT available in `DuckDB`-WASM. ⇒ [`WasmClass::NativeUdfOnly`].
    Udf {
        /// The registered UDF name (e.g. `"fossil_slug"`).
        udf_name: SmolStr,
    },
    /// Affects MIR/plan structure, not a scalar expression: the `seq/` operator
    /// family and the `io/` source constructors. Surface-unreachable in v0.1
    /// per ADR-0009 (registry-only here). ⇒ [`WasmClass::PureSql`].
    Plan(PlanOp),
}

/// The concrete inline SQL forms used by the v0.1 catalog. Each variant names
/// the SQL shape it expands to. Enum dispatch keeps codegen `Box<dyn>`-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineForm {
    /// `CAST(x AS <sql_type>)` — `parse.integer`→`BIGINT`, `parse.float`→`DOUBLE`,
    /// `parse.decimal`→`DECIMAL(38,18)`.
    Cast {
        /// The target `DuckDB` SQL type (e.g. `"BIGINT"`).
        sql_type: SmolStr,
    },
    /// `a || b || ...` — string concatenation operator (`core.triple` composite
    /// id, etc.).
    Concat,
    /// `'<value>'` — a fixed string literal, ignoring arguments. Used by
    /// `anon.redact` (default mask `[REDACTED]`).
    LiteralStr {
        /// The literal value to emit (without surrounding quotes).
        value: SmolStr,
    },
    /// `split_part(s, sep, n)` chain — `parse.csv_row`.
    SplitPart,
    /// `json_extract(s, path)` — `parse.json` (schema-directed parsing is
    /// post-surface-syntax; v0.1 lowers to a scalar extract).
    JsonExtract,
    /// `'_:bnode_' || row_id` — `core.blank`.
    BlankNode,
    /// `CASE WHEN x IS NULL THEN error(...) ELSE x END` — `core.require`.
    RequireNonNull,
    /// Identity passthrough — `core.iri` when its argument is already an IRI
    /// template; `core.literal`/`core.typed`/`core.lang` literal construction
    /// (the datatype/lang annotation is carried in a side column at codegen).
    Identity,
}

/// Plan-affecting operations: the `seq/` operator algebra family + `io/` sources.
///
/// These mirror the multi-input MIR ops (built and codegen-verified in Phase 4
/// via direct `MirGraph` construction, ADR-0009) but are surface-unreachable in
/// v0.1. Registry-only here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOp {
    /// `seq.filter` → `FilterOp` (`WHERE`).
    Filter,
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
    /// `seq.project` → `ProjectOp` (`SELECT cols`).
    Project,
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
    /// An `io/` source constructor. The tag mirrors `fossil-mir::SourceFormat`
    /// but is registry-local to avoid a dependency cycle on `fossil-mir`.
    Source(SourceFormatTag),
}

/// Registry-local mirror of `fossil-mir::SourceFormat` (avoids a `fossil-mir`
/// dependency / cycle). Phase 5 STDL-06 (plan 05-04) reconciles the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormatTag {
    /// `io.csv` → `read_csv_auto`.
    Csv,
    /// `io.json` → `read_json_auto`.
    Json,
    /// `io.parquet` → `read_parquet`.
    Parquet,
}

/// STDL-07 / SC#1 classification: does this function compile to pure `DuckDB` SQL
/// (OK in the browser playground), or does it require a native Rust UDF
/// (native-only, disabled in `DuckDB`-WASM)?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmClass {
    /// Compiles to pure `DuckDB` SQL — runs identically native and in WASM.
    PureSql,
    /// Requires a native Rust UDF — unavailable in the browser playground.
    NativeUdfOnly,
}

/// Derive the [`WasmClass`] from a [`LoweringKind`], enforcing the
/// `PureSql` ⟺ non-Udf invariant. Returns [`WasmClass::NativeUdfOnly`] iff the
/// lowering is [`LoweringKind::Udf`]; everything else is [`WasmClass::PureSql`].
///
/// Every catalog insertion sets `wasm_class` through this single function so the
/// two fields cannot drift (SC#1 / STDL-07).
#[must_use]
const fn derive_wasm_class(l: &LoweringKind) -> WasmClass {
    if matches!(l, LoweringKind::Udf { .. }) {
        WasmClass::NativeUdfOnly
    } else {
        WasmClass::PureSql
    }
}

impl FunctionRegistry {
    /// Phase-1 compatibility alias. Delegates to [`Self::stdlib_default`] so the
    /// historical four entries (`io.csv`, `core.iri`, `core.triple`,
    /// `core.literal`) remain present with the new shape and the walking
    /// skeleton / `fossil-mir` source recognition keep working unchanged.
    #[must_use]
    pub fn phase1_default() -> Self {
        Self::stdlib_default()
    }

    /// Construct the full Phase-5 stdlib catalog: every function across the
    /// eight surface namespaces (`seq`/`core`/`clean`/`parse`/`math`/`str`/
    /// `validate`/`anon`), reconciled to `stdlib.md` exactly, plus the `io/`
    /// source constructors. Each entry is classified `PureSql` or
    /// `NativeUdfOnly` via [`derive_wasm_class`].
    #[must_use]
    #[allow(clippy::too_many_lines)] // a flat catalog table; one line per stdlib fn.
    pub fn stdlib_default() -> Self {
        use InlineForm as IF;
        use LoweringKind as L;
        use PlanOp as P;
        use ScalarTy as S;

        let mut reg = Self {
            entries: HashMap::new(),
        };

        // Local insertion helper: sets `wasm_class` via derive_wasm_class so the
        // PureSql ⟺ non-Udf invariant is enforced at every call site.
        let add = |entries: &mut HashMap<SmolStr, RegistryEntry>,
                   name: &str,
                   params: Vec<ScalarTy>,
                   ret: ScalarTy,
                   lowering: LoweringKind| {
            let wasm_class = derive_wasm_class(&lowering);
            entries.insert(
                SmolStr::new(name),
                RegistryEntry {
                    name: SmolStr::new(name),
                    sig: SigSpec::new(params, ret),
                    lowering,
                    wasm_class,
                },
            );
        };
        let e = &mut reg.entries;

        // ── core/ (8) — foundational RDF primitives ───────────────────────
        // iri: validated cast; codegen identity when arg is already an IRI
        // template (stdlib.md §core/iri). v0.1 lowers as Inline(Identity).
        add(
            e,
            "core.iri",
            vec![S::String],
            S::Iri,
            L::Inline(IF::Identity),
        );
        add(
            e,
            "core.triple",
            vec![S::Iri, S::Iri, S::Iri],
            S::TripleTerm,
            L::Inline(IF::Concat),
        );
        // blank: () -> Iri / String -> Iri. v0.1 takes the named (String) form.
        add(
            e,
            "core.blank",
            vec![S::String],
            S::Iri,
            L::Inline(IF::BlankNode),
        );
        add(
            e,
            "core.literal",
            vec![S::String, S::Iri],
            S::String,
            L::Inline(IF::Identity),
        );
        add(
            e,
            "core.typed",
            vec![S::String, S::Iri],
            S::String,
            L::Inline(IF::Identity),
        );
        add(
            e,
            "core.lang",
            vec![S::String, S::String],
            S::String,
            L::Inline(IF::Identity),
        );
        add(
            e,
            "core.emit",
            vec![S::Iri, S::Iri, S::Iri],
            S::TripleTerm,
            L::Plan(P::Map),
        );
        // require: forall T. T? -> T. v0.1 scalar approximation String -> String.
        add(
            e,
            "core.require",
            vec![S::String],
            S::String,
            L::Inline(IF::RequireNonNull),
        );

        // ── seq/ (13) — source pipeline ops; all Plan ⇒ PureSql (STDL-01) ──
        // Higher-order Fn(...) args collapse to scalar placeholders in v0.1
        // (surface pipeline syntax deferred, ADR-0009). Signatures are
        // (String) -> String approximations; the PlanOp tag is the real datum.
        add(
            e,
            "seq.filter",
            vec![S::String],
            S::String,
            L::Plan(P::Filter),
        );
        add(e, "seq.map", vec![S::String], S::String, L::Plan(P::Map));
        add(
            e,
            "seq.flatten",
            vec![S::SeqString],
            S::String,
            L::Plan(P::Flatten),
        );
        add(
            e,
            "seq.take",
            vec![S::String, S::Integer],
            S::String,
            L::Plan(P::Take),
        );
        add(
            e,
            "seq.drop",
            vec![S::String, S::Integer],
            S::String,
            L::Plan(P::Drop),
        );
        add(
            e,
            "seq.distinct",
            vec![S::String],
            S::String,
            L::Plan(P::Distinct),
        );
        add(e, "seq.sort", vec![S::String], S::String, L::Plan(P::Sort));
        add(
            e,
            "seq.project",
            vec![S::String],
            S::String,
            L::Plan(P::Project),
        );
        add(
            e,
            "seq.join",
            vec![S::String, S::String],
            S::String,
            L::Plan(P::Join),
        );
        add(
            e,
            "seq.union",
            vec![S::String, S::String],
            S::String,
            L::Plan(P::Union),
        );
        add(
            e,
            "seq.group_by",
            vec![S::String],
            S::String,
            L::Plan(P::GroupBy),
        );
        add(
            e,
            "seq.aggregate",
            vec![S::String],
            S::String,
            L::Plan(P::Aggregate),
        );
        add(
            e,
            "seq.count",
            vec![S::String],
            S::Integer,
            L::Plan(P::Count),
        );

        // ── clean/ (6) — trim/lower/upper builtin; rest UDF ────────────────
        add(e, "clean.trim", vec![S::String], S::String, builtin("trim"));
        add(
            e,
            "clean.lower",
            vec![S::String],
            S::String,
            builtin("lower"),
        );
        add(
            e,
            "clean.upper",
            vec![S::String],
            S::String,
            builtin("upper"),
        );
        add(
            e,
            "clean.slug",
            vec![S::String],
            S::String,
            udf("fossil_slug"),
        );
        add(
            e,
            "clean.normalize_unicode",
            vec![S::String, S::String],
            S::String,
            udf("fossil_unicode_norm"),
        );
        add(
            e,
            "clean.strip_html",
            vec![S::String],
            S::String,
            udf("fossil_strip_html"),
        );

        // ── parse/ (7) — casts + strptime + json + csv_row; all PureSql ────
        add(
            e,
            "parse.integer",
            vec![S::String],
            S::Integer,
            L::Inline(IF::Cast {
                sql_type: SmolStr::new("BIGINT"),
            }),
        );
        add(
            e,
            "parse.float",
            vec![S::String],
            S::Float,
            L::Inline(IF::Cast {
                sql_type: SmolStr::new("DOUBLE"),
            }),
        );
        // decimal: no Decimal type in MVP → returns Float (stdlib.md §parse/decimal).
        add(
            e,
            "parse.decimal",
            vec![S::String],
            S::Float,
            L::Inline(IF::Cast {
                sql_type: SmolStr::new("DECIMAL(38,18)"),
            }),
        );
        add(
            e,
            "parse.date",
            vec![S::String, S::String],
            S::Date,
            builtin("strptime"),
        );
        add(
            e,
            "parse.datetime",
            vec![S::String, S::String],
            S::DateTime,
            builtin("strptime"),
        );
        // json: forall T. (String, schema) -> T. Schema-directed parsing is
        // post-surface-syntax (RESEARCH Open Q4); v0.1 lowers to a scalar
        // json_extract returning String.
        add(
            e,
            "parse.json",
            vec![S::String],
            S::String,
            L::Inline(IF::JsonExtract),
        );
        add(
            e,
            "parse.csv_row",
            vec![S::String, S::String],
            S::String,
            L::Inline(IF::SplitPart),
        );

        // ── math/ (6 — EXACTLY; NO ceil/floor) ─────────────────────────────
        add(e, "math.sum", vec![S::Float], S::Float, builtin("sum"));
        add(e, "math.avg", vec![S::Float], S::Float, builtin("avg"));
        add(e, "math.min", vec![S::Float], S::Float, builtin("min"));
        add(e, "math.max", vec![S::Float], S::Float, builtin("max"));
        add(e, "math.abs", vec![S::Float], S::Float, builtin("abs"));
        add(
            e,
            "math.round",
            vec![S::Float],
            S::Integer,
            builtin("round"),
        );

        // ── str/ (8) — DuckDB string builtins; all PureSql ─────────────────
        add(
            e,
            "str.length",
            vec![S::String],
            S::Integer,
            builtin("length"),
        );
        add(
            e,
            "str.slice",
            vec![S::String, S::Integer],
            S::String,
            builtin("substring"),
        );
        add(
            e,
            "str.contains",
            vec![S::String, S::String],
            S::Bool,
            builtin("contains"),
        );
        add(
            e,
            "str.starts_with",
            vec![S::String, S::String],
            S::Bool,
            builtin("starts_with"),
        );
        add(
            e,
            "str.ends_with",
            vec![S::String, S::String],
            S::Bool,
            builtin("ends_with"),
        );
        add(
            e,
            "str.replace",
            vec![S::String, S::String, S::String],
            S::String,
            builtin("replace"),
        );
        add(
            e,
            "str.split",
            vec![S::String, S::String],
            S::SeqString,
            builtin("string_split"),
        );
        add(
            e,
            "str.concat",
            vec![S::String, S::String],
            S::String,
            builtin("concat"),
        );

        // ── validate/ (5) — email/url/uuid/iso_date UDF; regex builtin ─────
        add(
            e,
            "validate.email",
            vec![S::String],
            S::String,
            udf("fossil_validate_email"),
        );
        add(
            e,
            "validate.url",
            vec![S::String],
            S::String,
            udf("fossil_validate_url"),
        );
        add(
            e,
            "validate.uuid",
            vec![S::String],
            S::String,
            udf("fossil_validate_uuid"),
        );
        add(
            e,
            "validate.iso_date",
            vec![S::String],
            S::String,
            udf("fossil_validate_iso_date"),
        );
        // regex: regex-expressible ⇒ PureSql (stdlib.md §validate/regex).
        add(
            e,
            "validate.regex",
            vec![S::String, S::String],
            S::String,
            builtin("regexp_matches"),
        );

        // ── anon/ (3) — hash (sha256 builtin), hmac (udf), redact (inline) ──
        // hash default (sha256, no salt) is the pure-SQL DuckDB builtin; salted
        // / blake3 variants are the `hmac` UDF entry (stdlib.md §anon/hash).
        add(
            e,
            "anon.hash",
            vec![S::String],
            S::String,
            builtin("sha256"),
        );
        add(
            e,
            "anon.hmac",
            vec![S::String, S::String],
            S::String,
            udf("fossil_hmac"),
        );
        // redact: default mask "[REDACTED]" — a fixed inline literal ⇒ PureSql.
        add(
            e,
            "anon.redact",
            vec![S::String],
            S::String,
            L::Inline(IF::LiteralStr {
                value: SmolStr::new("[REDACTED]"),
            }),
        );

        // ── io/ (3 in v0.1) — source constructors; Plan ⇒ PureSql (STDL-06) ─
        // NOT part of the eight-namespace surface-function completeness set.
        // io.sql / io.http are out of scope this milestone.
        add(
            e,
            "io.csv",
            vec![S::String],
            S::String,
            L::Plan(P::Source(SourceFormatTag::Csv)),
        );
        add(
            e,
            "io.json",
            vec![S::String],
            S::String,
            L::Plan(P::Source(SourceFormatTag::Json)),
        );
        add(
            e,
            "io.parquet",
            vec![S::String],
            S::String,
            L::Plan(P::Source(SourceFormatTag::Parquet)),
        );

        reg
    }

    /// Lookup a function entry by fully-qualified dotted name.
    /// Returns `None` if the name is unknown.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.get(name)
    }

    /// Iterate every registered entry (unspecified order). Used by 05-03's UDF
    /// manifest enumeration and 05-07's SC#1 CI classification gate.
    pub fn iter(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.entries.values()
    }
}

/// Helper: a `DuckDB` builtin lowering by name.
fn builtin(name: &str) -> LoweringKind {
    LoweringKind::Builtin {
        duckdb_name: SmolStr::new(name),
    }
}

/// Helper: a native Rust UDF lowering by name.
fn udf(name: &str) -> LoweringKind {
    LoweringKind::Udf {
        udf_name: SmolStr::new(name),
    }
}

#[cfg(test)]
mod tests;
