//! **What went wrong, as a value** — the catalogue of every mistake fossil
//! reports, compile diagnostics and run failures alike, and the [`Failure`]
//! that carries one out of a run.
//!
//! A code is `area/kind` and is the serde tag of [`Problem`], so the code and
//! the data cannot disagree and the message is rendered from the data by the
//! variant's one `#[error]`. The reasoning, the stability rule and the whole
//! catalogue are `/docs/design/errors`; this module holds the codes that have
//! an emitter today. A compile diagnostic carries one as
//! `fossil_base::Diagnostic::problem`; the same mistake found by `check` and
//! again by `run` is one code.
//!
//! `problem.schema.json` beside this crate's manifest is `Problem`'s derived
//! JSON Schema, held by `tests/problem_schema.rs`, and it is what the
//! TypeScript union is generated from.

use std::error::Error;
use std::fmt;

use schemars::JsonSchema;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::Span;

/// How bad a diagnostic or a failure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// A figure for a person, in MiB.
#[allow(clippy::cast_precision_loss, clippy::trivially_copy_pass_by_ref)]
fn mib(bytes: &u64) -> f64 {
    *bytes as f64 / f64::from(1_u32 << 20)
}

/// `` `a`, `b` `` — or `none` for an empty list.
fn code_list(items: &[String]) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn property_name(name: Option<&str>) -> String {
    name.map_or_else(
        || "this property has no name on its left, so it is not written".to_string(),
        |n| {
            format!(
                "`{n}` is not a property name: a property is named by a bare name, the last \
                 segment of a predicate IRI the shape declares"
            )
        },
    )
}

fn unknown_property(property: &str, declared: &[String]) -> String {
    if declared.is_empty() {
        format!("the target shape declares no predicate, so there is no `{property}`")
    } else {
        format!(
            "the target shape declares no `{property}` — it declares {}",
            code_list(declared)
        )
    }
}

fn unknown_function(function: &str, unknown_namespace: Option<&str>) -> String {
    unknown_namespace.map_or_else(
        || format!("`{function}` is not a function fossil knows"),
        |ns| {
            format!(
                "`{ns}` is not a namespace or a type fossil knows, so `{function}` names nothing"
            )
        },
    )
}

fn unknown_source(binding: &str, bound_to: Option<&str>) -> String {
    bound_to.map_or_else(
        || format!("`{binding}` is not a declared source binding"),
        |c| {
            format!(
                "`{binding}` is not a source: it is bound to `{c}`, which is not an `io.*(\"…\")` \
                 call, so there is no file to read"
            )
        },
    )
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn arity(function: &str, min: &u64, max: &u64, given: &u64) -> String {
    let takes = if min == max {
        format!("{min} argument{}", if *min == 1 { "" } else { "s" })
    } else {
        format!("{min} to {max} arguments")
    };
    let verb = if *given == 1 { "was" } else { "were" };
    format!("`{function}` takes {takes}, and {given} {verb} given")
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn duplicate(function: &str, parameter: &str, receiver: &bool) -> String {
    if *receiver {
        format!(
            "`{parameter}` is the receiver of `{function}` — the value to the left of the dot — \
             so naming it here gives it twice"
        )
    } else {
        format!("`{function}` is given `{parameter}` twice")
    }
}

fn argument_mismatch(
    function: &str,
    position: Option<&u64>,
    expected: &str,
    actual: &str,
) -> String {
    position.map_or_else(
        || {
            let member = function.rsplit('.').next().unwrap_or(function);
            format!("`{member}` is a member of {expected}, and this is {actual}")
        },
        |n| format!("argument {n} of `{function}` expects {expected}, and this is {actual}"),
    )
}

fn no_document(binding: Option<&str>) -> String {
    binding.map_or_else(
        || {
            "this program names no shape document, so it cannot write a property: a property \
             key is the last segment of a predicate IRI that a shape declares"
                .to_string()
        },
        |b| format!("`{b}` names no shape document, so it binds no shape"),
    )
}

fn unparseable(document: &str, reason: Option<&str>) -> String {
    reason.map_or_else(
        || format!("the document `{document}` does not parse"),
        |r| format!("the document `{document}` does not parse: {r}"),
    )
}

fn invalid_reader_option(option: &str, constructor: &str, value: Option<&str>) -> String {
    match value {
        None => format!(
            "`{option}` in `{constructor}` is written `{option} = \"<one character>\"`, and \
             this is not a string"
        ),
        Some("") => {
            format!("`{option}` in `{constructor}` is one ASCII character, and this is empty")
        }
        Some(v) => format!(
            "`{option}` in `{constructor}` is one ASCII character, and `\"{v}\"` is {} bytes",
            v.len()
        ),
    }
}

fn stage_arity(verb: &str, pipeline: &str, missing: Option<&str>) -> String {
    missing.map_or_else(
        || format!("`{verb}` in `{pipeline}` is given more arguments than it takes"),
        |m| format!("`{verb}` in `{pipeline}` needs {m}, and this call gives none"),
    )
}

fn not_an_aggregate(name: &str, pipeline: &str, function: Option<&str>) -> String {
    function.map_or_else(
        || {
            format!(
                "`{name}` in `{pipeline}` is not an aggregation: it has to be a call over the \
                 group"
            )
        },
        |f| {
            format!(
                "`{f}` is not an aggregate, so it cannot be the `{name}` of a `group_by` in \
                 `{pipeline}`"
            )
        },
    )
}

fn union_mismatch(
    pipeline: &str,
    left: &[String],
    right: &[String],
    column: Option<&u64>,
) -> String {
    let at = |side: &[String], i: u64| {
        usize::try_from(i)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| side.get(i))
            .map_or_else(|| "missing".to_string(), |c| format!("`{c}`"))
    };
    column.map_or_else(
        || {
            format!(
                "`union` in `{pipeline}` needs both sides to carry the same row: the left has \
                 {}, the right has {}",
                code_list(left),
                code_list(right)
            )
        },
        |&i| {
            format!(
                "`union` in `{pipeline}` pairs its sides column by column, and column {i} is {} \
                 on the left and {} on the right",
                at(left, i),
                at(right, i)
            )
        },
    )
}

/// Every variant states its code and its title once; the macro emits the
/// enum, [`Problem::code`], [`Problem::title`] and [`CODES`] from that one
/// statement, so none of the four can list a code the others do not.
macro_rules! catalogue {
    ($(
        $(#[doc = $doc:literal])*
        $code:literal, $title:literal,
        #[error($($message:tt)*)]
        $variant:ident { $($(#[$fattr:meta])* $field:ident : $ty:ty),* $(,)? }
    ),* $(,)?) => {
        /// **One mistake fossil can report**, identified by its code.
        ///
        /// Serialized as `{ "code": "area/kind", "data": { … } }`: the code is
        /// the serde tag, and `data` is always an object. `Display` is the
        /// message for a person, rendered from the data; nothing should parse
        /// it — a value a host needs is a field.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "code", content = "data")]
        pub enum Problem {
            $(
                $(#[doc = $doc])*
                #[serde(rename = $code)]
                #[schemars(title = $title)]
                #[error($($message)*)]
                $variant { $($(#[$fattr])* $field: $ty),* },
            )*
        }

        impl Problem {
            /// The code — `area/kind`, the serde tag. Stable once released:
            /// never reworded, never reused.
            #[must_use]
            pub const fn code(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $code,)*
                }
            }

            /// The title — fixed per code, short, sentence case, no trailing
            /// period (RFC 9457 §3.1.3). May be reworded in any release.
            #[must_use]
            pub const fn title(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $title,)*
                }
            }
        }

        /// Every live code, in declaration order.
        pub const CODES: &[&str] = &[$($code),*];

        /// Each code's `#[error]` as written — the format string and its
        /// arguments, as source text. `problem.schema.json` carries it as
        /// `x-detail`, and `cargo xtask problem` turns the ones it can read
        /// into the TypeScript that renders the same detail from the same data.
        pub const TEMPLATES: &[(&str, &str)] = &[$(($code, stringify!($($message)*))),*];
    };
}

catalogue! {
    // ── syntax ──────────────────────────────────────────────────────────

    /// The parser wanted one token and found another. Both are token kinds.
    "syntax/expected-token", "Expected another token",
    #[error("expected {expected}, found {found}")]
    ExpectedToken { expected: String, found: String },

    /// A token the parser skipped while recovering from an earlier mistake.
    "syntax/unexpected-token", "Unexpected token",
    #[error("unexpected token")]
    UnexpectedToken {},

    /// A character no token of the language starts with.
    "syntax/unknown-character", "No token starts with this character",
    #[error("unexpected character `{character}` — no token starts with it")]
    UnknownCharacter { character: String },

    /// A spelling the language had and retired, recognised on purpose so the
    /// message can name what replaces it.
    "syntax/retired-spelling", "A retired spelling",
    #[error("`{spelling}` is retired: write `{replacement}`")]
    RetiredSpelling { spelling: String, replacement: String },

    /// A `@rename` written incompletely; `missing` is the part it lacks.
    "syntax/malformed-rename", "A malformed rename",
    #[error(
        "this `@rename` has no {missing}: a rename is \
         `@rename(Type, \"<predicate IRI>\" as <name>)`"
    )]
    MalformedRename { missing: String },

    /// An attribute written where it does not belong.
    "syntax/misplaced-attribute", "An attribute in the wrong place",
    #[error("`{attribute}` does not belong {place}")]
    MisplacedAttribute { attribute: String, place: String },

    /// A mapping whose header cannot be read; nothing is produced from it.
    "syntax/invalid-mapping-header", "Not a mapping header",
    #[error(
        "this is not a mapping header — {reason} — so nothing is produced from it. A mapping is \
         `Name : Shape from <source>`"
    )]
    InvalidMappingHeader { reason: String },

    /// A property whose left-hand side is not a bare name; `name` is absent
    /// when there is nothing on the left at all.
    "syntax/invalid-property-name", "Not a property name",
    #[error("{}", property_name(.name.as_deref()))]
    InvalidPropertyName { name: Option<String> },

    /// A number literal that does not fit the type it is read as.
    "syntax/number-out-of-range", "A number fossil cannot carry",
    #[error("`{literal}` does not fit in {kind}: {reason}")]
    NumberOutOfRange { literal: String, kind: String, reason: String },

    /// A `? :` missing one of its three parts.
    "syntax/incomplete-conditional", "An incomplete conditional",
    #[error(
        "`{expression}` is not a complete conditional: it needs a condition, a `?` branch and \
         a `:` branch"
    )]
    IncompleteConditional { expression: String },

    /// A function named where a value belongs and never called.
    "syntax/uncalled-function", "A function named and not called",
    #[error("`{function}` names a function but does not call it; fossil has no function values")]
    UncalledFunction { function: String },

    /// A call whose callee is not a name.
    "syntax/invalid-callee", "Not a function name",
    #[error("`{callee}` calls something that is not a function name; only a catalogued name may be called")]
    InvalidCallee { callee: String },

    // ── name ────────────────────────────────────────────────────────────

    /// A shape name no `type { … } := …` binding introduces. `declared` is
    /// the names the program does bind.
    "name/unknown-shape", "Not a shape this program declares",
    #[error("`{shape}` is not a shape this program declares")]
    UnknownShape { shape: String, declared: Vec<String> },

    /// A property key the target shape does not declare.
    "name/unknown-property", "Not a property the shape declares",
    #[error("{}", unknown_property(.property, .declared))]
    UnknownProperty { property: String, declared: Vec<String> },

    /// A column a row does not have. `fields` is what it does have.
    "name/unknown-field", "Not a field of this row",
    #[error("`{field}` is not a field of `{relation}`")]
    UnknownField { field: String, relation: String, fields: Vec<String> },

    /// A qualified reference to a row that is not in scope where it is
    /// written — a mapping's, a pipeline stage's or a join's. `rows` is what
    /// is in scope.
    "name/row-not-in-scope", "A row that is not in scope",
    #[error("`{binding}.{column}` reads a row `{scope}` does not have — it has {}", code_list(.rows))]
    RowNotInScope { binding: String, column: String, scope: String, rows: Vec<String> },

    /// A call to a name the catalogue does not have. `unknown_namespace` is
    /// set when the part before the dot is itself unknown.
    "name/unknown-function", "Not a function fossil knows",
    #[error("{}", unknown_function(.function, .unknown_namespace.as_deref()))]
    UnknownFunction { function: String, unknown_namespace: Option<String> },

    /// A named argument no parameter of the function has.
    "name/unknown-parameter", "Not a parameter of this function",
    #[error("`{function}` has no parameter called `{parameter}` — it takes {}", code_list(.parameters))]
    UnknownParameter { function: String, parameter: String, parameters: Vec<String> },

    /// A `from` that names no source binding. `bound_to` is the constructor
    /// the name is bound to instead, when it is bound at all.
    "name/unknown-source", "Not a source binding",
    #[error("{}", unknown_source(.binding, .bound_to.as_deref()))]
    UnknownSource { binding: String, bound_to: Option<String> },

    // ── argument ────────────────────────────────────────────────────────

    /// A call given fewer arguments than it requires or more than it takes.
    "argument/arity", "The wrong number of arguments",
    #[error("{}", arity(.function, .min, .max, .given))]
    Arity { function: String, min: u64, max: u64, given: u64 },

    /// A parameter given twice — named twice, or named when the receiver
    /// already fills it.
    "argument/duplicate", "An argument given twice",
    #[error("{}", duplicate(.function, .parameter, .receiver))]
    DuplicateArgument { function: String, parameter: String, receiver: bool },

    /// A parameter left empty with a later one given.
    "argument/skipped-parameter", "A parameter skipped",
    #[error(
        "`{function}` is given nothing for `{parameter}`, and something after it; a parameter \
         cannot be skipped"
    )]
    SkippedParameter { function: String, parameter: String },

    /// A positional argument after a named one.
    "argument/positional-after-named", "A positional argument after a named one",
    #[error(
        "`{argument}` is positional and follows `{named} = …`; once an argument is named, the \
         ones after it are too"
    )]
    PositionalAfterNamed { argument: String, named: String },

    /// A named argument to an edge, whose arguments are positional.
    "argument/named-on-edge", "A named argument to an edge",
    #[error(
        "`{argument}` names an argument of `{target}`, and an edge's arguments fill its \
         identity template in the order they are written"
    )]
    NamedOnEdge { argument: String, target: String },

    /// A source alias given to a call that takes values.
    "argument/alias-in-value-call", "A source alias in a value call",
    #[error("`{alias}` is a source alias, and `{function}` takes values; an alias belongs to a join over rows")]
    AliasInValueCall { alias: String, function: String },

    // ── type ────────────────────────────────────────────────────────────

    /// A property written with a value of a type its shape does not allow.
    "type/property-mismatch", "A property of the wrong type",
    #[error("`{property}` expects {expected}, and this is {actual}")]
    PropertyMismatch { property: String, expected: String, actual: String },

    /// An argument of a type its parameter does not take. `position` is
    /// 1-based, and absent for the receiver of a member call.
    "type/argument-mismatch", "An argument of the wrong type",
    #[error("{}", argument_mismatch(.function, .position.as_ref(), .expected, .actual))]
    ArgumentMismatch { function: String, position: Option<u64>, expected: String, actual: String },

    /// A relation written where a value belongs.
    "type/relation-as-value", "A relation where a value belongs",
    #[error("`{function}` gives back a relation, which is not a value")]
    RelationAsValue { function: String },

    /// A value that has to be Bool and is not. `operand` says which.
    "type/expected-bool", "Not a Bool",
    #[error("{operand} must be Bool, and it is {actual}")]
    ExpectedBool { operand: String, actual: String },

    /// A value that has to be a number and is not. `operand` says which.
    "type/expected-number", "Not a number",
    #[error("{operand} must be a number, and it is {actual}")]
    ExpectedNumber { operand: String, actual: String },

    /// A comparison between two types that do not compare.
    "type/incomparable", "Values that cannot be compared",
    #[error("cannot compare {left} with {right} using `{operator}`")]
    Incomparable { left: String, right: String, operator: String },

    /// A `? :` whose branches have different types; fossil does not coerce.
    "type/branch-mismatch", "Branches of different types",
    #[error("the branches of `? :` have different types: {then} and {otherwise}")]
    BranchMismatch { then: String, otherwise: String },

    // ── shape ───────────────────────────────────────────────────────────

    /// No shape document where one is needed. `binding` is the binding that
    /// names none; absent when the program names none at all.
    "shape/no-document", "No shape document",
    #[error("{}", no_document(.binding.as_deref()))]
    NoDocument { binding: Option<String> },

    /// Two predicates of one shape whose short names coincide, which makes
    /// both unwritable. `first` and `second` are their IRIs.
    "shape/name-collision", "Two predicates share a name",
    #[error("two predicates of {shape} are both called `{name}`")]
    NameCollision { shape: String, name: String, first: String, second: String },

    /// A predicate the shape requires and the mapping never writes.
    "shape/missing-required-property", "A required property is never written",
    #[error("`{mapping}` never writes `{property}`, and {shape} requires it")]
    MissingRequiredProperty { mapping: String, property: String, shape: String },

    /// A value disjunction in a shape, which one mapping cannot write.
    "shape/unsupported-disjunction", "A value disjunction",
    #[error("a value disjunction is not supported: shape `{shape}` has {branches} branches")]
    UnsupportedDisjunction { shape: String, branches: u64 },

    /// A shape graph that refers back to itself.
    "shape/cyclic-reference", "A cyclic shape reference",
    #[error("cyclic shape graph not supported: {}", .path.join(" -> "))]
    CyclicReference { path: Vec<String> },

    /// A shape reference the document does not resolve.
    "shape/unresolved-reference", "An unresolved shape reference",
    #[error("unresolved shape reference `{reference}` in shape `{shape}`")]
    UnresolvedReference { reference: String, shape: String },

    /// A binding that names more shapes than its document declares; names
    /// bind by position, and `position` is the surplus name's.
    "shape/binding-arity", "More names than shapes",
    #[error(
        "the binding names {named} shape(s) and the document declares {declared}, so `{name}`, \
         at position {position}, binds nothing"
    )]
    BindingArity { name: String, named: u64, declared: u64, position: u64 },

    /// A `@rename` naming a type its binding does not introduce.
    "shape/rename-unknown-type", "A rename of a type the binding does not introduce",
    #[error("`@rename` names `{shape}`, which this `type` binding does not introduce")]
    RenameUnknownType { shape: String, introduced: Vec<String> },

    /// A `@rename` naming a predicate its shape does not declare.
    "shape/rename-unknown-predicate", "A rename of a predicate the shape does not declare",
    #[error("`{shape}` declares no predicate `{predicate}`, so this rename never fires")]
    RenameUnknownPredicate { shape: String, predicate: String, declared: Vec<String> },

    /// A program whose sources declare two different output shapes; one
    /// program writes one.
    "shape/more-than-one-output", "More than one output shape",
    #[error("a program may declare only one output shape; found `{first}` and `{second}`")]
    MoreThanOneOutput { first: String, second: String },

    // ── document ────────────────────────────────────────────────────────

    /// A document the program names that nothing is registered under.
    "document/not-registered", "A document is not registered",
    #[error("the document `{document}` is not registered")]
    NotRegistered { document: String },

    /// A registered document its provider could not decode. `reason` is the
    /// decoder's own account, when it gave one.
    "document/unparseable", "A document does not parse",
    #[error("{}", unparseable(.document, .reason.as_deref()))]
    Unparseable { document: String, reason: Option<String> },

    /// Documents a run needed that the host could not read.
    "document/unread", "Documents could not be read",
    #[error("{} document(s) could not be read: {}", .documents.len(), .documents.join(", "))]
    Unread { documents: Vec<String> },

    // ── provider ────────────────────────────────────────────────────────

    /// A constructor no installed provider answers to.
    "provider/unknown", "Not a provider this host installs",
    #[error("`{constructor}` is not a provider this host installs")]
    UnknownProvider { constructor: String },

    /// A provider asked for something it does not do — types of a row reader.
    "provider/wrong-capability", "The provider cannot do this",
    #[error("`{constructor}` does not {capability}")]
    WrongCapability { constructor: String, capability: String },

    /// A document whose extension the provider does not read.
    "provider/wrong-extension", "The provider does not read this extension",
    #[error("`{constructor}` does not read `{document}`")]
    WrongExtension { constructor: String, document: String },

    /// A document written as a bare path, with no provider to read it.
    "provider/bare-document-path", "A document path with no provider",
    #[error("`{document}` is named by no provider")]
    BareDocumentPath { document: String },

    /// A reader option whose value no reader can be given: not a string
    /// (`value` absent), or not one ASCII character.
    "provider/invalid-reader-option", "A reader option fossil cannot pass on",
    #[error("{}", invalid_reader_option(.option, .constructor, .value.as_deref()))]
    InvalidReaderOption { option: String, constructor: String, value: Option<String> },

    /// A reader option the provider does not have; `owner` is the one that does.
    "provider/foreign-reader-option", "A reader option of another provider",
    #[error("`{constructor}` has no `{option}`; `{owner}` does")]
    ForeignReaderOption { option: String, constructor: String, owner: String },

    // ── identity ────────────────────────────────────────────────────────

    /// A mapping with no `@subject`.
    "identity/missing-subject", "A mapping with no identity",
    #[error("`{mapping}` declares no `@subject`, so the rows it writes have no identity")]
    MissingSubject { mapping: String },

    /// A mapping with two `@subject` lines.
    "identity/duplicate-subject", "Two identities in one mapping",
    #[error("`{mapping}` declares `@subject` twice, and a type has one identity")]
    DuplicateSubject { mapping: String },

    /// A `@subject` that is not the first line of its body. `line` is
    /// 1-based, within the body.
    "identity/subject-not-first", "The identity is not first",
    #[error("`@subject` is the first line of a mapping body, and in `{mapping}` it is line {line}")]
    SubjectNotFirst { mapping: String, line: u64 },

    /// Two mappings producing one type with two different `@subject` forms.
    "identity/conflicting", "Two identities for one type",
    #[error("`{first}` and `{second}` mint two identities for {shape}")]
    ConflictingIdentity { first: String, second: String, shape: String },

    /// An edge to a type no mapping writes, so there is no identity to build.
    "identity/edge-without-template", "An edge to a type nothing writes",
    #[error(
        "no mapping in this program writes a `{target}`, so `{target}(…)` has no identity \
         template to build from"
    )]
    EdgeWithoutTemplate { target: String },

    /// An edge given a number of values its identity template has no holes
    /// for. `mapping` is the one whose `@subject` declares the template.
    "identity/edge-arity", "An edge given the wrong number of values",
    #[error(
        "`{target}` is built from {takes} value(s) and this passes {given}; its identity is \
         declared by `{mapping}`"
    )]
    EdgeArity { target: String, takes: u64, given: u64, mapping: String },

    // ── pipeline ────────────────────────────────────────────────────────

    /// A stage whose verb the catalogue does not have.
    "pipeline/unknown-verb", "Not a relation verb",
    #[error("`{verb}` is not a relation verb; the catalogue has {}", code_list(.verbs))]
    UnknownVerb { verb: String, verbs: Vec<String> },

    /// A verb the catalogue declares and the lowering does not implement yet.
    "pipeline/unimplemented-verb", "A verb with no lowering yet",
    #[error(
        "`{verb}` is a relation verb the lowering does not implement yet, so `{pipeline}` \
         cannot compile; implemented today: {}",
        code_list(.implemented)
    )]
    UnimplementedVerb { verb: String, pipeline: String, implemented: Vec<String> },

    /// A stage missing a position it needs (`missing`), or given more than
    /// it takes (`missing` absent).
    "pipeline/stage-arity", "A stage given the wrong arguments",
    #[error("{}", stage_arity(.verb, .pipeline, .missing.as_deref()))]
    StageArity { verb: String, pipeline: String, missing: Option<String> },

    /// A stage argument of the wrong kind — a value where a column or a
    /// binding name belongs.
    "pipeline/invalid-stage-argument", "A stage argument of the wrong kind",
    #[error("`{parameter}` in `{pipeline}` takes {expected}, and this is not one")]
    InvalidStageArgument { parameter: String, pipeline: String, expected: String },

    /// A `group_by` output that is not an aggregate call. `function` is the
    /// call written, when it is a call; `aggregates` are the ones there are.
    "pipeline/not-an-aggregate", "Not an aggregation",
    #[error("{}", not_an_aggregate(.name, .pipeline, .function.as_deref()))]
    NotAnAggregate { name: String, pipeline: String, function: Option<String>, aggregates: Vec<String> },

    /// An alias on a `union`, whose result has the pipeline's one name.
    "pipeline/union-alias", "An alias on a union",
    #[error(
        "`union` in `{pipeline}` gives its result the one name `{pipeline}`, so the alias \
         `{alias}` would name a row it does not produce"
    )]
    UnionAlias { pipeline: String, alias: String },

    /// A pipeline that derives from itself.
    "pipeline/cycle", "A pipeline that derives from itself",
    #[error(
        "the source pipeline `{pipeline}` derives from itself, directly or through the \
         pipelines it names"
    )]
    PipelineCycle { pipeline: String },

    /// A `join` whose other side declares no schema, so its condition cannot
    /// be checked.
    "pipeline/unknown-columns", "A join over a source with no schema",
    #[error(
        "`join` in `{pipeline}` joins {}, whose columns are unknown: it declares no schema, so \
         there is nothing to check the condition against",
        code_list(.bindings)
    )]
    SchemalessJoin { pipeline: String, bindings: Vec<String> },

    /// A `union` whose two sides carry different rows. Each column is
    /// `name: Type`; `column` is the 1-based first that differs, absent when
    /// the counts do.
    "pipeline/union-mismatch", "Union sides with different rows",
    #[error("{}", union_mismatch(.pipeline, .left, .right, .column.as_ref()))]
    UnionMismatch { pipeline: String, left: Vec<String>, right: Vec<String>, column: Option<u64> },

    /// A `join` condition that is not an equality, or a conjunction of them.
    "pipeline/join-not-equality", "A join condition that is not an equality",
    #[error("`join` in `{pipeline}` relates its two sides by equality, and `{condition}` is not one")]
    JoinNotEquality { pipeline: String, condition: String },

    /// A `join` key that is not a column reference.
    "pipeline/join-key-not-column", "A join key that is not a column",
    #[error(
        "`join` in `{pipeline}` equates `{left}` with `{right}`, and `{operand}` is not a column \
         reference"
    )]
    JoinKeyNotColumn { pipeline: String, left: String, right: String, operand: String },

    /// A `join` equality whose two columns are on the same side.
    "pipeline/join-one-side", "A join condition on one side only",
    #[error("`join` in `{pipeline}` equates `{left}` with `{right}`, and both are columns of the {side} side")]
    JoinOneSide { pipeline: String, left: String, right: String, side: String },

    // ── unsupported ─────────────────────────────────────────────────────

    /// An expression or literal the parser reads and the lowering has no case
    /// for — a gap in the compiler, not a mistake in the program.
    "unsupported/expression", "An expression fossil cannot lower yet",
    #[error("`{expression}` is not an expression fossil can lower yet, so this property is not written")]
    UnsupportedExpression { expression: String },

    // ── run ─────────────────────────────────────────────────────────────

    /// An operator asked for more memory than the run's budget had left.
    /// Raised while the graph executes, before any byte of the corpus is
    /// written. Sizes are bytes.
    "run/over-budget", "The run needs more memory than its budget",
    #[error(
        "the run needs more memory than the executor's {:.0} MiB budget: {consumer} asked for \
         {:.1} MiB with {:.1} MiB already held. Nothing was written.",
        mib(.budget), mib(.requested), mib(.reserved)
    )]
    OverBudget { consumer: String, requested: u64, reserved: u64, budget: u64 },

    /// A run over a program that does not compile. The failure's `related`
    /// carries the program's diagnostics.
    "run/does-not-compile", "The program does not compile",
    #[error("the program does not compile; nothing was run")]
    DoesNotCompile {},

    /// The destination a run writes under is not a prefix any store covers.
    "run/destination-uncovered", "No store covers the destination",
    #[error("no store covers the destination {destination}")]
    DestinationUncovered { destination: String },

    // ── source, engine, write, layout, storage, corpus, api ─────────────

    /// A source's bytes are not UTF-8 text.
    "source/not-utf8", "A source is not UTF-8",
    #[error("the source `{locator}` is not UTF-8")]
    NotUtf8 { locator: String },

    /// A source's bytes did not parse in the format the program named.
    "source/unparseable", "A source does not parse",
    #[error("the source `{locator}` does not parse")]
    SourceUnparseable { locator: String },

    /// The query engine failed; its own error is the cause.
    "engine/failed", "The query engine failed",
    #[error("the query engine failed")]
    EngineFailed {},

    /// Writing one file of the corpus failed; the store's error is the cause.
    "write/failed", "A corpus file could not be written",
    #[error("`{path}` could not be written")]
    WriteFailed { path: String },

    /// More vertices than a `u32` `dense_id` can number.
    "layout/too-large", "The graph has too many vertices",
    #[error("the graph has {vertices} vertices; a `dense_id` is a u32")]
    TooLarge { vertices: u64 },

    /// The host vended no credential for a scope.
    "storage/no-credential", "No credential was vended",
    #[error("the host vended no {access} credential for {scope}")]
    NoCredential { scope: String, access: String },

    /// A scope whose credentials cover more than the one prefix a reader needs.
    "storage/ambiguous-prefix", "More than one prefix was vended",
    #[error("{scope} vends {count} prefixes, and one was expected")]
    AmbiguousPrefix { scope: String, count: u64 },

    /// A locator outside every prefix a credential was vended for.
    "storage/outside-prefix", "Outside the vended prefix",
    #[error("{locator} lies outside {prefix}")]
    OutsidePrefix { locator: String, prefix: String },

    /// A vended credential fossil cannot read: a missing or malformed key, or
    /// a prefix that is not a directory.
    "storage/malformed-credential", "A credential is malformed",
    #[error("the credential for {prefix} is malformed: `{key}` {reason}")]
    MalformedCredential { prefix: String, key: String, reason: String },

    /// A locator no kind of store fossil reads can route.
    "storage/no-route", "No store routes this locator",
    #[error(
        "{locator} has no route: fossil reads s3://bucket/… and \
         abfss://container@account.dfs.core.windows.net/… through a vended credential, and \
         http(s) without one"
    )]
    NoRoute { locator: String },

    /// A store answered with an error; the store's error is the cause.
    "storage/unreachable", "A store could not be reached",
    #[error("{locator} could not be reached")]
    Unreachable { locator: String },

    /// The engine a host handed over cannot read remote storage.
    "storage/no-httpfs", "The engine cannot read remote storage",
    #[error("the engine cannot read remote storage: its httpfs extension is not loaded")]
    NoHttpfs {},

    /// The host refused a request for credentials; its rejection is the cause.
    "storage/host-refused", "The host refused",
    #[error("the host refused {scope}")]
    HostRefused { scope: String },

    /// The host did not answer a request for credentials or connections within
    /// the deadline fossil holds every host wait to; `after` is that deadline,
    /// in milliseconds.
    "storage/host-silent", "The host did not answer",
    #[error("the host did not answer {scope} within {after} ms")]
    HostSilent { scope: String, after: u64 },

    /// A wasm module that could not be fetched — refused, missing, or not
    /// there within the boot deadline; `after` is that deadline, in
    /// milliseconds, when it is what ended the wait. The browser's error is
    /// the cause.
    "module/unreachable", "A module could not be loaded",
    #[error("{locator} could not be loaded")]
    ModuleUnreachable {
        locator: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        after: Option<u64>,
    },

    /// A corpus whose manifest could not be read; the reader's error is the cause.
    "corpus/unreadable", "The corpus could not be read",
    #[error("{path} could not be read")]
    CorpusUnreadable { path: String },

    /// A manifest that is not JSON.
    "corpus/not-json", "The manifest is not JSON",
    #[error("{path} is not JSON")]
    NotJson { path: String },

    /// A manifest declaring a format this reader does not read.
    "corpus/unsupported-format", "The corpus format is not supported",
    #[error("{path} declares format {format}, and this reader reads fossil/1 only")]
    UnsupportedFormat { path: String, format: String },

    /// A manifest declaring one table twice.
    "corpus/duplicate-table", "A table is declared twice",
    #[error("the corpus declares {table} twice")]
    DuplicateTable { table: String },

    /// A corpus location carrying a query or a fragment. A corpus is a prefix
    /// and every file is named under it; a query string — a signature among
    /// them — addresses one object and does not survive the join. Signed
    /// storage is reached by opening a job under the host that vends it.
    "corpus/not-a-location", "Not a corpus location",
    #[error(
        "{location} carries a query or a fragment, and a corpus location is a prefix: signed \
         storage is opened as a job, under the host that vends its credential"
    )]
    NotALocation { location: String },

    /// A table the corpus does not have.
    "corpus/unknown-table", "Not a table of this corpus",
    #[error("{table} is not a table of this corpus — its tables are {}", .tables.join(", "))]
    UnknownTable { table: String, tables: Vec<String> },

    /// A column a table does not declare.
    "corpus/unknown-column", "Not a column of this table",
    #[error("{table} does not declare {column} — its columns are {}", .columns.join(", "))]
    UnknownColumn { table: String, column: String, columns: Vec<String> },

    /// A projection that selects no column.
    "corpus/empty-projection", "The projection selects nothing",
    #[error("the projection names no column of {table}")]
    EmptyProjection { table: String },

    /// A spatial filter over a table that declares no position.
    "corpus/no-position", "The table has no position",
    #[error("{table} declares no position, so it has no box to filter by")]
    NoPosition { table: String },

    /// A filter comparing a column with a value of another type.
    "corpus/filter-type-mismatch", "The filter compares different types",
    #[error("{column} holds {holds}, and the filter compares it with {value}")]
    FilterTypeMismatch { column: String, holds: String, value: String },

    /// An argument a caller passed that is not what the call takes.
    "api/invalid-argument", "An argument is invalid",
    #[error("`{argument}` is not {expected}")]
    InvalidArgument { argument: String, expected: String },

    /// A call made while another call on the same object is still running.
    "api/busy", "Another call is running",
    #[error("`{call}` was called while another call is running")]
    Busy { call: String },

    /// A handle that names nothing — dropped, or never issued.
    "api/unknown-handle", "The handle names nothing",
    #[error("the handle names nothing: it was dropped, or never issued")]
    UnknownHandle {},

    /// Fossil's own fault, not the caller's. Nobody branches on it; a host
    /// shows it and reports it.
    "internal/bug", "An internal error",
    #[error("internal error: {what}")]
    Bug { what: String },
}

/// Codes that were live once and are not now, with the tag that last emitted
/// each. A code is never reused, and a test holds that no live code is here.
pub const RETIRED: &[(&str, &str)] = &[
    ("write/unknown-type", "v0.3.0-alpha.18"),
    ("layout/dangling-endpoint", "v0.3.0-alpha.18"),
];

/// The published documentation, and the error index within it — `DOCS` and
/// `INDEX` in `@fossil-lang/types`' `error.ts`, held equal by a test below.
const DOCS: &str = "https://kanzo-tech.github.io/fossil-lang";
const INDEX: &str = "docs/errors";

impl Problem {
    /// The published page that explains this code — `helpUrl(code)` in
    /// `@fossil-lang/types`. What an LSP `codeDescription.href` points at; a
    /// host serving its own copy of the site builds the link with `helpUrl`.
    #[must_use]
    pub fn help_url(&self) -> String {
        format!("{DOCS}/{INDEX}/{}", self.code())
    }
}

/// A compile diagnostic carried by a run failure — what
/// [`Problem::DoesNotCompile`] relates. The span is **file-absolute**.
///
/// `fossil_base::Diagnostic` cannot be named here: that crate depends on this
/// one and is the salsa accumulator. This is its positional part reduced to
/// the span, and the same [`Problem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub severity: Severity,
    pub problem: Problem,
    pub help: Option<String>,
    pub span: Option<Span>,
}

impl fmt::Display for Related {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.problem, f)
    }
}

impl Error for Related {}

/// `{ code, data, title, detail, severity, help?, span? }` — a problem, as
/// [`Failure`] writes one, plus where.
impl Serialize for Related {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            #[serde(flatten)]
            problem: &'a Problem,
            title: &'static str,
            detail: String,
            severity: Severity,
            #[serde(skip_serializing_if = "Option::is_none")]
            help: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            span: Option<Span>,
        }
        Wire {
            problem: &self.problem,
            title: self.problem.title(),
            detail: self.problem.to_string(),
            severity: self.severity,
            help: self.help.as_deref(),
            span: self.span,
        }
        .serialize(serializer)
    }
}

impl miette::Diagnostic for Related {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.problem.code()))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
            Severity::Info => miette::Severity::Advice,
        })
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help.as_ref().map(|h| Box::new(h) as _)
    }
}

/// Whether `code` is `area/kind`: two lowercase kebab-case words joined by a
/// slash, a digit allowed after a word's first letter (`source/not-utf8`).
fn is_code(code: &str) -> bool {
    let word = |w: &str| {
        w.starts_with(|c: char| c.is_ascii_lowercase())
            && w.split('-').all(|p| {
                !p.is_empty()
                    && p.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
    };
    code.split_once('/')
        .is_some_and(|(area, kind)| word(area) && word(kind))
}

/// **An error fossil did not raise**, kept whole as a [`Failure`]'s cause.
///
/// `DataFusion`, `object_store` and a host each have their own vocabulary and
/// fossil does not classify it; `name` is the error's own name — its Rust type's
/// last path segment, or the name a JavaScript host gave it — and `Display` is
/// its own text. The original, when there is one, is [`Error::source`].
///
/// A host's error may carry its own vocabulary's `code`, `area/kind`, and the
/// `data` that code carries: they ride along so a host reads its own code back
/// off the chain. Neither is one of fossil's, and fossil never branches on them.
#[derive(Debug)]
pub struct Foreign {
    name: String,
    detail: String,
    /// Boxed: most foreign errors carry no code, and every `Result` that holds
    /// a `Foreign` pays for the field's width.
    coded: Option<Box<Coded>>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

#[derive(Debug)]
struct Coded {
    code: String,
    data: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Foreign {
    /// Wrap `error`, named after its type.
    pub fn new<E: Error + Send + Sync + 'static>(error: E) -> Self {
        let name = std::any::type_name::<E>();
        let name = name.split('<').next().unwrap_or(name);
        let name = name.rsplit("::").next().unwrap_or(name).to_owned();
        Self {
            name,
            detail: error.to_string(),
            coded: None,
            source: Some(Box::new(error)),
        }
    }

    /// A cause that arrives only as a name and a text — a JavaScript host's
    /// rejection.
    pub fn named(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: detail.into(),
            coded: None,
            source: None,
        }
    }

    /// The producer's own `code`, and the `data` it carries. A code that is not
    /// `area/kind` is not kept, and neither is its data.
    #[must_use]
    pub fn coded(
        mut self,
        code: impl Into<String>,
        data: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Self {
        let code = code.into();
        if is_code(&code) {
            self.coded = Some(Box::new(Coded { code, data }));
        }
        self
    }

    /// The error's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The producer's own code — never one of fossil's, whatever it spells.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        self.coded.as_ref().map(|c| c.code.as_str())
    }

    /// What [`Self::code`] carries.
    #[must_use]
    pub fn data(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.coded.as_ref().and_then(|c| c.data.as_ref())
    }
}

impl fmt::Display for Foreign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl Error for Foreign {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|e| e as _)
    }
}

/// **A failure that is not about a position in a program**: a [`Problem`],
/// the help this occurrence has, the diagnostics it relates, and the error
/// that caused it.
#[derive(Debug)]
pub struct Failure {
    pub problem: Problem,
    pub help: Option<String>,
    pub related: Vec<Related>,
    pub cause: Option<Box<dyn Error + Send + Sync>>,
}

impl Failure {
    #[must_use]
    pub const fn new(problem: Problem) -> Self {
        Self {
            problem,
            help: None,
            related: Vec::new(),
            cause: None,
        }
    }

    /// What this occurrence can say about repairing it.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// The diagnostics this failure is about.
    #[must_use]
    pub fn with_related(mut self, related: Vec<Related>) -> Self {
        self.related = related;
        self
    }

    /// The error that caused this one. A [`Failure`] or a [`Foreign`] is kept
    /// as it is; any other error is wrapped in a [`Foreign`] named after its
    /// type, and stays reachable whole through [`Error::source`].
    #[must_use]
    pub fn caused_by<E: Error + Send + Sync + 'static>(mut self, cause: E) -> Self {
        let boxed: Box<dyn Error + Send + Sync> = Box::new(cause);
        self.cause = Some(if boxed.is::<Self>() || boxed.is::<Foreign>() {
            boxed
        } else {
            match boxed.downcast::<E>() {
                Ok(cause) => Box::new(Foreign::new(*cause)),
                Err(boxed) => boxed,
            }
        });
        self
    }
}

impl From<Problem> for Failure {
    fn from(problem: Problem) -> Self {
        Self::new(problem)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.problem, f)
    }
}

impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_deref().map(|e| e as _)
    }
}

impl miette::Diagnostic for Failure {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.problem.code()))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Error)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help.as_ref().map(|h| Box::new(h) as _)
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn miette::Diagnostic> + 'a>> {
        if self.related.is_empty() {
            return None;
        }
        Some(Box::new(
            self.related.iter().map(|r| r as &dyn miette::Diagnostic),
        ))
    }
}

/// The wire shape of `/docs/design/errors#the-boundary-one-shape-and-a-real-error`:
/// `{ code, data, title, detail, severity, help?, related?, cause? }`, where a
/// cause is a nested failure or `{ name, detail, code?, data? }` — told apart
/// by `name`, which a failure never has.
impl Serialize for Failure {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            #[serde(flatten)]
            problem: &'a Problem,
            title: &'static str,
            detail: String,
            severity: Severity,
            #[serde(skip_serializing_if = "Option::is_none")]
            help: Option<&'a str>,
            #[serde(skip_serializing_if = "<[Related]>::is_empty")]
            related: &'a [Related],
            #[serde(skip_serializing_if = "Option::is_none")]
            cause: Option<Cause<'a>>,
        }
        struct Cause<'a>(&'a (dyn Error + Send + Sync + 'static));
        impl Serialize for Cause<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                if let Some(failure) = self.0.downcast_ref::<Failure>() {
                    return failure.serialize(serializer);
                }
                let foreign = self.0.downcast_ref::<Foreign>();
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("name", foreign.map_or("Error", Foreign::name))?;
                map.serialize_entry("detail", &self.0.to_string())?;
                if let Some(code) = foreign.and_then(Foreign::code) {
                    map.serialize_entry("code", code)?;
                }
                if let Some(data) = foreign.and_then(Foreign::data) {
                    map.serialize_entry("data", data)?;
                }
                map.end()
            }
        }
        Wire {
            problem: &self.problem,
            title: self.problem.title(),
            detail: self.problem.to_string(),
            severity: Severity::Error,
            help: self.help.as_deref(),
            related: &self.related,
            cause: self.cause.as_deref().map(Cause),
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn samples() -> Vec<Problem> {
        vec![
            Problem::OverBudget {
                consumer: "ExternalSorter".into(),
                requested: 1 << 20,
                reserved: 3 << 20,
                budget: 2 << 30,
            },
            Problem::DoesNotCompile {},
            Problem::DestinationUncovered {
                destination: "s3://lake/out/".into(),
            },
            Problem::UnknownTable {
                table: "Pet".into(),
                tables: vec!["Person".into()],
            },
            Problem::NoHttpfs {},
            Problem::Bug { what: "x".into() },
            Problem::UnknownField {
                field: "nmae".into(),
                relation: "User".into(),
                fields: vec!["name".into()],
            },
            Problem::Unparseable {
                document: "shop.shex".into(),
                reason: None,
            },
            Problem::ArgumentMismatch {
                function: "str.trim".into(),
                position: None,
                expected: "String".into(),
                actual: "Integer".into(),
            },
        ]
    }

    #[test]
    fn an_optional_field_with_no_value_is_absent_not_null() {
        let quiet = Problem::ModuleUnreachable {
            locator: "m_bg.wasm".into(),
            after: None,
        };
        let wire = serde_json::to_value(&quiet).expect("serializes");
        assert_eq!(wire["data"], serde_json::json!({ "locator": "m_bg.wasm" }));
        assert_eq!(
            serde_json::from_value::<Problem>(wire).expect("reads back"),
            quiet
        );
        let late = Problem::ModuleUnreachable {
            locator: "m_bg.wasm".into(),
            after: Some(60_000),
        };
        let wire = serde_json::to_value(&late).expect("serializes");
        assert_eq!(wire["data"]["after"], 60_000);
    }

    #[test]
    fn every_code_is_area_slash_kind_and_listed_once() {
        let mut seen = HashSet::new();
        for code in CODES {
            assert!(is_code(code), "{code} is not area/kind");
            assert!(seen.insert(code), "{code} is listed twice");
        }
    }

    #[test]
    fn no_live_code_is_retired() {
        for (code, _) in RETIRED {
            assert!(!CODES.contains(code), "{code} is live and retired");
        }
    }

    #[test]
    fn the_code_is_the_serde_tag_and_data_is_an_object() {
        for problem in samples() {
            let json = serde_json::to_value(&problem).expect("serialises");
            assert_eq!(json["code"], problem.code(), "{json}");
            assert!(json["data"].is_object(), "{json}");
            assert!(CODES.contains(&problem.code()));
            assert!(!problem.title().ends_with('.'), "{}", problem.title());
            let back: Problem = serde_json::from_value(json).expect("round-trips");
            assert_eq!(back, problem);
        }
    }

    #[test]
    fn the_message_is_rendered_from_the_data() {
        let problem = &samples()[0];
        assert_eq!(
            problem.to_string(),
            "the run needs more memory than the executor's 2048 MiB budget: ExternalSorter asked \
             for 1.0 MiB with 3.0 MiB already held. Nothing was written."
        );
    }

    #[test]
    fn the_wire_nests_a_failure_and_names_a_foreign_cause() {
        let inner = Failure::new(Problem::Unreachable {
            locator: "s3://lake/a".into(),
        })
        .caused_by(std::fmt::Error);
        let outer = Failure::new(Problem::EngineFailed {})
            .with_help("try again")
            .caused_by(inner);
        let wire = serde_json::to_value(&outer).expect("serialises");
        assert_eq!(wire["code"], "engine/failed");
        assert_eq!(wire["title"], "The query engine failed");
        assert_eq!(wire["severity"], "error");
        assert_eq!(wire["help"], "try again");
        assert!(wire.get("related").is_none());
        assert_eq!(wire["cause"]["code"], "storage/unreachable");
        assert_eq!(wire["cause"]["data"]["locator"], "s3://lake/a");
        assert_eq!(wire["cause"]["cause"]["name"], "Error");
        assert_eq!(
            wire["cause"]["cause"]["detail"],
            std::fmt::Error.to_string()
        );

        let hosted = Failure::new(Problem::HostRefused {
            scope: "job 1".into(),
        })
        .caused_by(Foreign::named("AuthError", "no"));
        let wire = serde_json::to_value(&hosted).expect("serialises");
        assert_eq!(
            wire["cause"],
            serde_json::json!({ "name": "AuthError", "detail": "no" })
        );
    }

    #[test]
    fn a_foreign_cause_keeps_its_own_code_and_data() {
        let data = serde_json::json!({ "job": "j1" });
        let refused = Failure::new(Problem::HostRefused {
            scope: "job j1".into(),
        })
        .caused_by(
            Foreign::named("ApiError", "no such job")
                .coded("job/not-found", data.as_object().cloned()),
        );
        let wire = serde_json::to_value(&refused).expect("serialises");
        assert_eq!(
            wire["cause"],
            serde_json::json!({
                "name": "ApiError",
                "detail": "no such job",
                "code": "job/not-found",
                "data": { "job": "j1" },
            })
        );
    }

    #[test]
    fn a_code_off_the_grammar_is_not_kept() {
        for code in [
            "ECONNREFUSED",
            "ERR_INVALID_ARG_TYPE",
            "job",
            "Job/not-found",
            "job/",
        ] {
            let foreign = Foreign::named("Error", "x").coded(code, Some(serde_json::Map::new()));
            assert_eq!(foreign.code(), None, "{code}");
            assert_eq!(foreign.data(), None, "{code}");
        }
    }

    /// `help_url` and TypeScript's `helpUrl` are two spellings of one link; the
    /// constants are read out of `error.ts` rather than restated here.
    #[test]
    fn the_help_url_is_the_one_typescript_builds() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/types/src/error.ts"
        );
        let ts = std::fs::read_to_string(path).expect("packages/types/src/error.ts is readable");
        let constant = |name: &str| {
            ts.lines()
                .find_map(|l| l.strip_prefix(&format!("const {name} = '")))
                .and_then(|rest| rest.strip_suffix("';"))
                .unwrap_or_else(|| panic!("error.ts declares `const {name} = '…';`"))
                .to_string()
        };
        assert_eq!(constant("DOCS"), DOCS);
        assert_eq!(constant("INDEX"), INDEX);
        assert_eq!(
            Problem::Busy {
                call: "check".into()
            }
            .help_url(),
            "https://kanzo-tech.github.io/fossil-lang/docs/errors/api/busy"
        );
    }

    #[test]
    fn a_foreign_cause_keeps_the_original_reachable() {
        let failure = Failure::new(Problem::EngineFailed {}).caused_by(std::fmt::Error);
        let cause = failure.source().expect("a cause");
        let original = cause.source().expect("the original");
        assert!(original.downcast_ref::<std::fmt::Error>().is_some());
    }
}
