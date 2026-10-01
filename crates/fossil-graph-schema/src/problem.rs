//! **What went wrong, as a value** — the catalogue of every failure fossil
//! reports to a host, and the [`Failure`] that carries one.
//!
//! A code is `area/kind` and is the serde tag of [`Problem`], so the code and
//! the data cannot disagree and the message is rendered from the data by the
//! variant's one `#[error]`. The reasoning, the stability rule and the whole
//! catalogue are `/docs/design/errors`; this module holds the codes that have
//! an emitter today.
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

/// Every variant states its code and its title once; the macro emits the
/// enum, [`Problem::code`], [`Problem::title`] and [`CODES`] from that one
/// statement, so none of the four can list a code the others do not.
macro_rules! catalogue {
    ($(
        $(#[doc = $doc:literal])*
        $code:literal, $title:literal,
        #[error($($message:tt)*)]
        $variant:ident { $($field:ident : $ty:ty),* $(,)? }
    ),* $(,)?) => {
        /// **One failure fossil can report**, identified by its code.
        ///
        /// Serialized as `{ "code": "area/kind", "data": { … } }`: the code is
        /// the serde tag, and `data` is always an object. `Display` is the
        /// message for a person, rendered from the data; nothing should parse
        /// it — a value a host needs is a field.
        #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "code", content = "data")]
        pub enum Problem {
            $(
                $(#[doc = $doc])*
                #[serde(rename = $code)]
                #[schemars(title = $title)]
                #[error($($message)*)]
                $variant { $($field: $ty),* },
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
    };
}

catalogue! {
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

    /// A program whose sources declare two different output shapes; one
    /// program writes one.
    "shape/more-than-one-output", "More than one output shape",
    #[error("a program may declare only one output shape; found `{first}` and `{second}`")]
    MoreThanOneOutput { first: String, second: String },

    /// A shape document written as a bare path, with no provider to read it.
    /// Also a compile code; the executor reaches it for the output shape.
    "provider/bare-document-path", "A document path with no provider",
    #[error("the shape document `{document}` is named by no provider")]
    BareDocumentPath { document: String },

    /// A document the program names that the host never registered.
    "document/not-registered", "A document is not registered",
    #[error("the document `{document}` is not registered")]
    NotRegistered { document: String },

    /// A registered document its provider could not decode.
    "document/unparseable", "A document does not parse",
    #[error("the document `{document}` does not parse")]
    Unparseable { document: String },

    /// Documents a run needed that the host could not read.
    "document/unread", "Documents could not be read",
    #[error("{} document(s) could not be read: {}", .documents.len(), .documents.join(", "))]
    Unread { documents: Vec<String> },

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

    /// A relation naming a vertex type the graph did not materialise.
    "write/unknown-type", "A relation names a missing vertex type",
    #[error("relation `{relation}` references vertex type `{vertex_type}`, which has no table")]
    UnknownType { relation: String, vertex_type: String },

    /// A relation whose rows name vertices that do not exist.
    "layout/dangling-endpoint", "A relation names missing vertices",
    #[error("relation `{relation}` has {dropped} of {before} rows naming a vertex that does not exist")]
    DanglingEndpoint { relation: String, before: u64, dropped: u64 },

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
pub const RETIRED: &[(&str, &str)] = &[];

/// A compile diagnostic carried by a run failure — what
/// [`Problem::DoesNotCompile`] relates. The span is **file-absolute**.
///
/// `fossil_base::Diagnostic` cannot be named here: that crate depends on this
/// one and is the salsa accumulator. Step 5 of `/docs/design/errors` gives
/// this a `problem` as it gives one to `Diagnostic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Related {
    pub severity: Severity,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

impl fmt::Display for Related {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl Error for Related {}

impl miette::Diagnostic for Related {
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

/// **An error fossil did not raise**, kept whole as a [`Failure`]'s cause.
///
/// `DataFusion`, `object_store` and a host each have their own vocabulary and
/// fossil does not classify it; `name` is the error's own name — its Rust type's
/// last path segment, or the name a JavaScript host gave it — and `Display` is
/// its own text. The original, when there is one, is [`Error::source`].
#[derive(Debug)]
pub struct Foreign {
    name: String,
    detail: String,
    source: Option<Box<dyn Error + Send + Sync>>,
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
            source: Some(Box::new(error)),
        }
    }

    /// A cause that arrives only as a name and a text — a JavaScript host's
    /// rejection.
    pub fn named(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: detail.into(),
            source: None,
        }
    }

    /// The error's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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
/// cause is a nested failure or `{ name, detail }`.
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
                let name = self
                    .0
                    .downcast_ref::<Foreign>()
                    .map_or("Error", Foreign::name);
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("detail", &self.0.to_string())?;
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
        ]
    }

    /// `^[a-z][a-z0-9]*(-[a-z0-9]+)*/` the same `$`: lowercase kebab-case, a
    /// digit allowed after a word's first letter because `source/not-utf8` is one.
    fn well_formed(code: &str) -> bool {
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

    #[test]
    fn every_code_is_area_slash_kind_and_listed_once() {
        let mut seen = HashSet::new();
        for code in CODES {
            assert!(well_formed(code), "{code} is not area/kind");
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
    fn a_foreign_cause_keeps_the_original_reachable() {
        let failure = Failure::new(Problem::EngineFailed {}).caused_by(std::fmt::Error);
        let cause = failure.source().expect("a cause");
        let original = cause.source().expect("the original");
        assert!(original.downcast_ref::<std::fmt::Error>().is_some());
    }
}
