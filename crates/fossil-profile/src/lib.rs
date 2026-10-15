//! `fossil-profile` — what a source is: its columns and their types, how many
//! rows it has, which columns are candidate keys, which columns hold another
//! source's keys, and a few of its rows.
//!
//! # Sans-IO
//!
//! This crate decides which queries to run and reads their answers. It runs
//! nothing: [`Profiler::next_query`] hands a host a [`Query`], the host runs it
//! on its own `DuckDB` and gives the rows back with [`Profiler::feed`], and
//! [`Profiler::finish`] is the [`Profile`]. It is the shape of `h11` and
//! `quinn-proto`, for the reason those have it: two hosts — the language
//! server on native `DuckDB`, the browser on `DuckDB-WASM` — and one
//! implementation of the part that is logic. A `DESCRIBE` had no logic to agree
//! on, so `introspect` could be written twice; which pairs of columns to test
//! and what a count means are logic, and two copies of logic drift.
//!
//! # The plan is data
//!
//! Each query is an [`Ask`] — the counts of a relation, the commonest values of
//! a column, an inclusion between two, a sample — and SQL is one rendering of
//! it, [`Ask::sql`], `DuckDB`'s, which the two hosts that profile SOURCES run.
//! The graph writer counts the TABLES it wrote with the same plan
//! ([`Profiler::over_tables`]) on `DataFusion`, which has no SQL in this build,
//! and renders the same `Ask` as a `DataFusion` plan. A table plan stops at the
//! counts: a graph carries no inclusions (`/docs/design/facts`). Which queries to run, how
//! to prune them and what a count means stay here; only the rendering is per
//! engine (`/docs/design/facts`).
//!
//! Every query answers in text, so the engines answer in one form: rows of text
//! cells, `None` for SQL `NULL`.
//!
//! [`run`] is the loop for a host that runs one query at a time: it brings the
//! one thing a host owns, a function from SQL to rows, and this crate links no
//! engine — so it is in the wasm closure and the gate watches it
//! (`/docs/design/profile`).
//!
//! # What a profile claims
//!
//! A key and an inclusion read on a sample are **candidates**, not facts: a
//! column unique in the first rows may repeat later. [`Options::sample`] says how
//! many rows the counts read; `0` reads them whole. The referenced side of an
//! inclusion is always read whole, so a key that a sample cut short cannot
//! refuse a value that is there.

use std::collections::{HashMap, VecDeque};

use fossil_base::sources::{InferredColumn, InferredDescriptor};
use fossil_base::{NativeReader, RowReader};
use fossil_graph_schema::Primitive;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

pub mod generated;

/// A query's answer: its rows, each cell the text of a value or `None` for `NULL`.
pub type Rows = Vec<Vec<Option<String>>>;

/// The longest value a profile keeps, in bytes. A longer one is left out,
/// never cut: a prefix is a value the column does not hold, and a reader shows
/// these to a person and filters by them (`/docs/design/facts`). 64 bytes hold
/// a date, a number, a code or a short name, and not a description.
pub const MAX_VALUE_BYTES: usize = 64;

/// How many of a column's commonest values a profile keeps.
pub const TOP_VALUES: usize = 10;

/// A source to profile, as its host can read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ProfileSource")]
pub struct Source {
    /// The name the program binds it to (`users` in `users := io.csv(…)`):
    /// what every part of the profile that names a source names it by.
    #[schemars(with = "String")]
    pub binding: SmolStr,
    /// The reference as the program wrote it — what the descriptor is keyed
    /// by, so two bindings of one file share a descriptor.
    #[schemars(with = "String")]
    pub key: SmolStr,
    /// What the host's `DuckDB` reads: the location, or the name the host
    /// attached it under. Storage is the host's; this is only the name.
    pub location: String,
    /// The catalogue row its constructor names (`csv`), which picks the reader.
    pub format: String,
    /// The reader option the binding wrote (`delimiter = "|"`), verbatim.
    #[serde(default)]
    pub option: Option<String>,
    /// The host's validator for the bytes it read: carried into the descriptor.
    #[serde(default)]
    pub etag: String,
}

/// A table the host already holds, with no reader: the columns and their
/// types are known, so it is not described. What the graph writer counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The name the host's engine holds it under.
    pub name: SmolStr,
    pub columns: Vec<InferredColumn>,
}

/// How far a profile goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ProfileDepth")]
#[serde(rename_all = "lowercase")]
pub enum Depth {
    /// The columns and their types, and nothing else: one `DESCRIBE` a source.
    /// What a host that compiles on every keystroke can afford — the editor.
    Types,
    /// Types, counts, keys, inclusions and a sample.
    #[default]
    Whole,
}

/// How much of a source a profile reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ProfileOptions")]
#[serde(default)]
pub struct Options {
    /// The rows the counts read from each source; `0` reads every row.
    pub sample: u64,
    /// The rows kept as the profile's sample of each source.
    pub shown: u64,
    /// How far it goes: the types alone, or the whole profile.
    pub depth: Depth,
}

impl Options {
    /// The columns and their types alone: what the compiler's cache takes.
    pub const TYPES: Self = Self {
        sample: 0,
        shown: 0,
        depth: Depth::Types,
    };

    /// Every row, and no sample kept: what the graph writer counts with.
    pub const WHOLE: Self = Self {
        sample: 0,
        shown: 0,
        depth: Depth::Whole,
    };
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sample: 10_000,
            shown: 5,
            depth: Depth::Whole,
        }
    }
}

/// One query for the host to run, as `DuckDB` SQL — [`Ask::sql`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ProfileQuery")]
pub struct Query {
    /// What its rows are fed back with.
    pub id: u32,
    pub sql: String,
}

/// What a query reads: a source through its reader, or a table the host holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relation {
    /// `read_csv_auto('people.csv', delim='|')` — [`relation`]'s rendering.
    Read(String),
    /// A table the host's engine holds under this name.
    Table(SmolStr),
}

impl Relation {
    /// The relation in `DuckDB` SQL.
    #[must_use]
    pub fn sql(&self) -> String {
        match self {
            Self::Read(read) => read.clone(),
            Self::Table(name) => ident(name),
        }
    }
}

/// One query of the plan, as data. Each answers in text cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The relation's columns and their engine types: a row `[name, type]`
    /// per column.
    Describe { relation: Relation },
    /// One row: the rows read, then per column its distinct non-null values,
    /// its non-null values, its least and its greatest value.
    Counts {
        relation: Relation,
        /// The rows read; `0` reads every row.
        sample: u64,
        columns: Vec<SmolStr>,
    },
    /// A row `[value, count]` for each of the [`TOP_VALUES`] commonest
    /// non-null values that occur at least twice, most frequent first and ties
    /// by value.
    TopValues {
        relation: Relation,
        sample: u64,
        column: SmolStr,
    },
    /// One row: the distinct non-null values of `column`, and how many of them
    /// `key` does not hold. The referenced side is read whole.
    Inclusion {
        dependent: Relation,
        sample: u64,
        column: SmolStr,
        referenced: Relation,
        key: SmolStr,
    },
    /// The first `shown` rows, every cell as text.
    Sample {
        relation: Relation,
        columns: Vec<SmolStr>,
        shown: u64,
    },
}

impl Ask {
    /// `DuckDB`'s rendering — what both hosts that profile sources run.
    #[must_use]
    pub fn sql(&self) -> String {
        let read = |relation: &Relation, sample: u64| match sample {
            0 => format!("(SELECT * FROM {})", relation.sql()),
            n => format!("(SELECT * FROM {} LIMIT {n})", relation.sql()),
        };
        match self {
            Self::Describe { relation } => format!("DESCRIBE SELECT * FROM {}", relation.sql()),
            Self::Counts {
                relation,
                sample,
                columns,
            } => {
                let mut select = vec!["CAST(count(*) AS VARCHAR)".to_string()];
                for c in columns {
                    let name = ident(c);
                    select.push(format!("CAST(count(DISTINCT {name}) AS VARCHAR)"));
                    select.push(format!("CAST(count({name}) AS VARCHAR)"));
                    select.push(format!("CAST(min({name}) AS VARCHAR)"));
                    select.push(format!("CAST(max({name}) AS VARCHAR)"));
                }
                format!(
                    "SELECT {} FROM {} AS s",
                    select.join(", "),
                    read(relation, *sample)
                )
            }
            Self::TopValues {
                relation,
                sample,
                column,
            } => format!(
                "SELECT {text} AS v, CAST(count(*) AS VARCHAR) FROM {from} AS s \
                 WHERE {col} IS NOT NULL GROUP BY {col} HAVING count(*) >= 2 \
                 ORDER BY count(*) DESC, v LIMIT {TOP_VALUES}",
                text = text(column),
                col = ident(column),
                from = read(relation, *sample),
            ),
            Self::Inclusion {
                dependent,
                sample,
                column,
                referenced,
                key,
            } => format!(
                "SELECT CAST(count(*) AS VARCHAR), CAST(count(*) FILTER (WHERE y.v IS NULL) AS VARCHAR) \
                 FROM (SELECT DISTINCT {from} AS v FROM {dependent} AS a WHERE {col} IS NOT NULL) AS x \
                 LEFT JOIN (SELECT DISTINCT {to} AS v FROM {referenced} AS b) AS y ON x.v = y.v",
                from = text(column),
                col = ident(column),
                dependent = read(dependent, *sample),
                to = text(key),
                referenced = read(referenced, 0),
            ),
            Self::Sample {
                relation,
                columns,
                shown,
            } => {
                let select: Vec<String> = columns.iter().map(|c| text(c)).collect();
                format!(
                    "SELECT {} FROM {} LIMIT {shown}",
                    select.join(", "),
                    relation.sql()
                )
            }
        }
    }
}

/// A column's counts, over the rows the profile read — Parquet's `Statistics`
/// names, which the graph's chunk footers use for the same facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColumnStats {
    #[schemars(with = "String")]
    pub name: SmolStr,
    /// Rows with no value.
    pub null_count: u64,
    /// Distinct non-null values.
    pub distinct_count: u64,
    /// The least value, as text; absent with no value, or one longer than
    /// [`MAX_VALUE_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_value: Option<String>,
    /// The greatest value, on the same terms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_value: Option<String>,
    /// The commonest values that occur at least twice; absent when none
    /// repeats, or when one of them is longer than [`MAX_VALUE_BYTES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_values: Vec<TopValue>,
}

/// One of a column's commonest values, and how many rows hold it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TopValue {
    pub value: String,
    pub count: u64,
}

/// One source, profiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceProfile {
    /// The binding it was given: what [`ColumnRef::source`] and
    /// [`Failure::source`] name it by. The descriptor's `key` is the
    /// reference, which a second binding of the same file shares.
    #[schemars(with = "String")]
    pub binding: SmolStr,
    /// Its columns and their types: what the compiler's cache takes.
    pub descriptor: InferredDescriptor,
    /// The rows the counts read, and whether that was a sample.
    pub rows: Option<u64>,
    pub sampled: bool,
    pub columns: Vec<ColumnStats>,
    /// The columns that are candidate keys: of a type a key can be, never null,
    /// never repeated.
    #[schemars(with = "Vec<String>")]
    pub candidate_keys: Vec<SmolStr>,
    /// Its first rows, every cell as text, in the descriptor's column order.
    pub sample: Rows,
}

/// A column of one source and a column of the same or another.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ColumnRef {
    /// The source's binding.
    #[schemars(with = "String")]
    pub source: SmolStr,
    #[schemars(with = "String")]
    pub column: SmolStr,
}

/// Every value of `from` is a value of `to`, which is a candidate key: the
/// shape of an edge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Inclusion {
    pub from: ColumnRef,
    pub to: ColumnRef,
}

/// A query the host could not run, and what it said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ProfileFailure")]
pub struct Failure {
    /// The binding of the source it was about: for an inclusion, the dependent
    /// side's.
    #[schemars(with = "String")]
    pub source: SmolStr,
    pub reason: String,
}

/// What the profiler found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Profile {
    /// Every source that could be described, in the order given.
    pub sources: Vec<SourceProfile>,
    pub inclusions: Vec<Inclusion>,
    /// A source that could not be described is here and not in `sources`; a
    /// later query that failed leaves its source with less in it.
    pub failed: Vec<Failure>,
}

/// The reader a catalogue row reads with, if it reads through a native
/// `DuckDB` table function: a materialised row (`io.rdf`) takes its schema from
/// its shape and has nothing to describe.
#[must_use]
pub fn native_reader(format: &str) -> Option<NativeReader> {
    fossil_base::providers::DATA
        .iter()
        .find(|p| p.name == format)
        .and_then(|p| match p.reads_rows {
            Some(RowReader::Native(r)) => Some(r),
            _ => None,
        })
}

/// What `DuckDB` reads a source as: `read_csv_auto('people.csv', delim='|')`.
#[must_use]
pub fn relation(reader: NativeReader, name: &str, option: Option<&str>) -> String {
    let args = match (option, reader.option_keyword()) {
        (Some(value), Some(keyword)) => format!(", {keyword}='{}'", value.replace('\'', "''")),
        _ => String::new(),
    };
    format!(
        "{}('{}'{args})",
        reader.table_function(),
        name.replace('\'', "''")
    )
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn text(column: &str) -> String {
    format!("CAST({} AS VARCHAR)", ident(column))
}

/// A column a key can be: not a measurement, whose equality is exact.
const fn keyable(p: Primitive) -> bool {
    !matches!(p, Primitive::Float | Primitive::Bool)
}

/// A value the profile keeps: present, and no longer than [`MAX_VALUE_BYTES`].
fn kept(cell: Option<&Option<String>>) -> Option<String> {
    cell.cloned()
        .flatten()
        .filter(|v| v.len() <= MAX_VALUE_BYTES)
}

#[derive(Debug)]
struct State {
    binding: SmolStr,
    key: SmolStr,
    etag: String,
    format: Option<String>,
    relation: Relation,
    described: bool,
    columns: Vec<InferredColumn>,
    rows: Option<u64>,
    stats: Vec<ColumnStats>,
    sample: Rows,
}

#[derive(Debug, Clone, Copy)]
enum Pending {
    Describe(usize),
    Stats(usize),
    Top(usize, usize),
    Inclusion {
        from: (usize, usize),
        to: (usize, usize),
    },
    Sample(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Describe,
    Stats,
    Top,
    Inclusions,
    Sample,
    Done,
}

/// The plan: describe every source, count each one's columns, find the
/// commonest values of a column whose values repeat, test the inclusions the
/// types allow, keep a sample. A phase's queries are handed out together, so a
/// host may run them at once; the next phase begins once all of them are
/// answered.
#[derive(Debug)]
pub struct Profiler {
    options: Options,
    states: Vec<State>,
    phase: Phase,
    queue: VecDeque<(u32, Ask)>,
    pending: HashMap<u32, Pending>,
    next_id: u32,
    /// Whether the inclusions between the relations are tested: between
    /// sources, and never between the tables of a graph.
    between: bool,
    inclusions: Vec<Inclusion>,
    failed: Vec<Failure>,
}

impl Profiler {
    /// A plan over `sources`. One per binding: a program binds a name once, so
    /// a second source with a binding already given is the same source and is
    /// not read again. Two bindings of one file are two sources, each profiled
    /// under its own name. A source whose format reads through no native
    /// function is left out — there is nothing to describe.
    #[must_use]
    pub fn new(sources: impl IntoIterator<Item = Source>, options: Options) -> Self {
        let mut states: Vec<State> = Vec::new();
        for source in sources {
            let Some(reader) = native_reader(&source.format) else {
                continue;
            };
            if states.iter().any(|s| s.binding == source.binding) {
                continue;
            }
            let relation =
                Relation::Read(relation(reader, &source.location, source.option.as_deref()));
            states.push(State {
                binding: source.binding,
                key: source.key,
                etag: source.etag,
                format: Some(source.format),
                relation,
                described: false,
                columns: Vec::new(),
                rows: None,
                stats: Vec::new(),
                sample: Vec::new(),
            });
        }
        let mut plan = Self::with(states, options);
        for i in 0..plan.states.len() {
            let relation = plan.states[i].relation.clone();
            plan.ask(Ask::Describe { relation }, Pending::Describe(i));
        }
        plan
    }

    /// A plan over tables the host already holds, whose columns are known: no
    /// `DESCRIBE`, and no inclusion — the counts, the commonest values and the
    /// keys they make. What the graph writer counts what it wrote with: a
    /// graph carries no inclusions, which were measured and dropped
    /// (`/docs/design/facts`).
    #[must_use]
    pub fn over_tables(tables: impl IntoIterator<Item = Table>, options: Options) -> Self {
        let states = tables
            .into_iter()
            .map(|t| State {
                binding: t.name.clone(),
                key: t.name.clone(),
                etag: String::new(),
                format: None,
                relation: Relation::Table(t.name),
                described: true,
                columns: t.columns,
                rows: None,
                stats: Vec::new(),
                sample: Vec::new(),
            })
            .collect();
        Self {
            between: false,
            ..Self::with(states, options)
        }
    }

    fn with(states: Vec<State>, options: Options) -> Self {
        Self {
            options,
            states,
            phase: Phase::Describe,
            queue: VecDeque::new(),
            pending: HashMap::new(),
            next_id: 0,
            between: true,
            inclusions: Vec::new(),
            failed: Vec::new(),
        }
    }

    fn ask(&mut self, ask: Ask, what: Pending) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, what);
        self.queue.push_back((id, ask));
    }

    /// The next query to run as `DuckDB` SQL, or `None` when there is none to
    /// hand out now: either the profile is done, or the phase waits on answers
    /// still out.
    pub fn next_query(&mut self) -> Option<Query> {
        self.next_ask()
            .map(|(id, ask)| Query { id, sql: ask.sql() })
    }

    /// The next query to run, as data, for a host that renders it itself.
    pub fn next_ask(&mut self) -> Option<(u32, Ask)> {
        while self.queue.is_empty() && self.pending.is_empty() && self.phase != Phase::Done {
            self.advance();
        }
        self.queue.pop_front()
    }

    /// Whether every query has been answered and none is left to ask.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done && self.pending.is_empty()
    }

    fn advance(&mut self) {
        self.phase = match self.phase {
            Phase::Describe if self.options.depth == Depth::Types => Phase::Done,
            Phase::Describe => {
                for i in 0..self.states.len() {
                    let state = &self.states[i];
                    if state.columns.is_empty() {
                        continue;
                    }
                    let ask = Ask::Counts {
                        relation: state.relation.clone(),
                        sample: self.options.sample,
                        columns: state.columns.iter().map(|c| c.name.clone()).collect(),
                    };
                    self.ask(ask, Pending::Stats(i));
                }
                Phase::Stats
            }
            Phase::Stats => {
                // Only a column whose values repeat has commonest values: one
                // with as many distinct values as values has none to find.
                let mut asks = Vec::new();
                for (i, state) in self.states.iter().enumerate() {
                    for (c, stats) in state.stats.iter().enumerate() {
                        let present = state
                            .rows
                            .unwrap_or_default()
                            .saturating_sub(stats.null_count);
                        if stats.distinct_count < present {
                            asks.push((
                                Ask::TopValues {
                                    relation: state.relation.clone(),
                                    sample: self.options.sample,
                                    column: stats.name.clone(),
                                },
                                Pending::Top(i, c),
                            ));
                        }
                    }
                }
                for (ask, what) in asks {
                    self.ask(ask, what);
                }
                Phase::Top
            }
            Phase::Top => {
                if self.between {
                    self.plan_inclusions();
                }
                Phase::Inclusions
            }
            Phase::Inclusions => {
                for i in 0..self.states.len() {
                    let state = &self.states[i];
                    if state.columns.is_empty() || self.options.shown == 0 {
                        continue;
                    }
                    let ask = Ask::Sample {
                        relation: state.relation.clone(),
                        columns: state.columns.iter().map(|c| c.name.clone()).collect(),
                        shown: self.options.shown,
                    };
                    self.ask(ask, Pending::Sample(i));
                }
                Phase::Sample
            }
            Phase::Sample | Phase::Done => Phase::Done,
        };
    }

    /// Every pair the types allow: a dependent column with values, and a
    /// candidate key of another column of the same primitive.
    fn plan_inclusions(&mut self) {
        let mut asks = Vec::new();
        for (b, target) in self.states.iter().enumerate() {
            for key in self.candidate_keys(b) {
                let Some(k) = target.columns.iter().position(|c| c.name == key) else {
                    continue;
                };
                let primitive = target.columns[k].primitive;
                for (a, dependent) in self.states.iter().enumerate() {
                    for (c, column) in dependent.columns.iter().enumerate() {
                        let has_values = dependent.stats.get(c).is_some_and(|s| {
                            dependent.rows.is_some_and(|rows| s.null_count < rows)
                        });
                        if (a, c) == (b, k) || column.primitive != primitive || !has_values {
                            continue;
                        }
                        asks.push((
                            Ask::Inclusion {
                                dependent: dependent.relation.clone(),
                                sample: self.options.sample,
                                column: column.name.clone(),
                                referenced: target.relation.clone(),
                                key: key.clone(),
                            },
                            Pending::Inclusion {
                                from: (a, c),
                                to: (b, k),
                            },
                        ));
                    }
                }
            }
        }
        for (ask, what) in asks {
            self.ask(ask, what);
        }
    }

    /// Source `i`'s candidate keys: columns a key can be, never null and never
    /// repeated in the rows read — and there were rows.
    fn candidate_keys(&self, i: usize) -> Vec<SmolStr> {
        let state = &self.states[i];
        let Some(rows) = state.rows.filter(|&r| r > 0) else {
            return Vec::new();
        };
        state
            .columns
            .iter()
            .zip(&state.stats)
            .filter(|(c, s)| keyable(c.primitive) && s.null_count == 0 && s.distinct_count == rows)
            .map(|(c, _)| c.name.clone())
            .collect()
    }

    /// The rows query `id` answered.
    pub fn feed(&mut self, id: u32, rows: &[Vec<Option<String>>]) {
        let Some(what) = self.pending.remove(&id) else {
            return;
        };
        let count = |cell: Option<&Option<String>>| {
            cell.and_then(|c| c.as_deref())
                .and_then(|c| c.parse::<u64>().ok())
        };
        match what {
            Pending::Describe(i) => {
                let state = &mut self.states[i];
                let reader = state.format.as_deref().and_then(native_reader);
                state.described = true;
                state.columns = rows
                    .iter()
                    .filter_map(|row| {
                        let name = row.first()?.as_deref()?;
                        let kind = row.get(1)?.as_deref()?;
                        let primitive = reader
                            .map_or(Primitive::String, |r| generated::read_primitive(r, kind));
                        (!name.is_empty()).then(|| InferredColumn {
                            name: name.into(),
                            primitive,
                        })
                    })
                    .collect();
            }
            Pending::Stats(i) => {
                let Some(row) = rows.first() else { return };
                let state = &mut self.states[i];
                let Some(total) = count(row.first()) else {
                    return;
                };
                state.rows = Some(total);
                state.stats = state
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(n, c)| {
                        let at = 1 + 4 * n;
                        let present = count(row.get(at + 1)).unwrap_or(0);
                        ColumnStats {
                            name: c.name.clone(),
                            null_count: total.saturating_sub(present),
                            distinct_count: count(row.get(at)).unwrap_or(0),
                            min_value: kept(row.get(at + 2)),
                            max_value: kept(row.get(at + 3)),
                            top_values: Vec::new(),
                        }
                    })
                    .collect();
            }
            Pending::Top(i, c) => {
                let top: Option<Vec<TopValue>> = rows
                    .iter()
                    .take(TOP_VALUES)
                    .map(|row| {
                        Some(TopValue {
                            value: kept(row.first())?,
                            count: count(row.get(1))?,
                        })
                    })
                    .collect();
                if let Some(stats) = self.states[i].stats.get_mut(c) {
                    // One value too long and the list goes whole: ten values
                    // with a hole are not the ten commonest.
                    stats.top_values = top.unwrap_or_default();
                }
            }
            Pending::Inclusion { from, to } => {
                let Some(row) = rows.first() else { return };
                let (Some(values), Some(missing)) = (count(row.first()), count(row.get(1))) else {
                    return;
                };
                if values > 0 && missing == 0 {
                    let at = |(s, c): (usize, usize)| ColumnRef {
                        source: self.states[s].binding.clone(),
                        column: self.states[s].columns[c].name.clone(),
                    };
                    self.inclusions.push(Inclusion {
                        from: at(from),
                        to: at(to),
                    });
                }
            }
            Pending::Sample(i) => self.states[i].sample = rows.to_vec(),
        }
    }

    /// Query `id` could not be run, and `reason` is what the host said.
    pub fn fail(&mut self, id: u32, reason: impl Into<String>) {
        let Some(what) = self.pending.remove(&id) else {
            return;
        };
        let i = match what {
            Pending::Describe(i) | Pending::Stats(i) | Pending::Sample(i) | Pending::Top(i, _) => i,
            Pending::Inclusion { from: (a, _), .. } => a,
        };
        self.failed.push(Failure {
            source: self.states[i].binding.clone(),
            reason: reason.into(),
        });
    }

    /// The profile. Called before [`Profiler::is_done`], it holds what was
    /// answered so far.
    #[must_use]
    pub fn finish(self) -> Profile {
        let keys: Vec<Vec<SmolStr>> = (0..self.states.len())
            .map(|i| self.candidate_keys(i))
            .collect();
        let sampled = self.options.sample;
        let sources = self
            .states
            .into_iter()
            .zip(keys)
            .filter(|(s, _)| s.described)
            .map(|(s, candidate_keys)| SourceProfile {
                binding: s.binding,
                sampled: sampled > 0 && s.rows.is_some_and(|r| r >= sampled),
                rows: s.rows,
                columns: s.stats,
                candidate_keys,
                sample: s.sample,
                descriptor: InferredDescriptor {
                    key: s.key,
                    columns: s.columns,
                    etag: s.etag,
                },
            })
            .collect();
        Profile {
            sources,
            inclusions: self.inclusions,
            failed: self.failed,
        }
    }
}

/// The whole plan over `sources`, each query run by `exec` — the host's engine,
/// one query at a time. A query `exec` refuses is a [`Failure`] in the profile,
/// and the plan goes on.
pub fn run(
    sources: impl IntoIterator<Item = Source>,
    options: Options,
    mut exec: impl FnMut(&str) -> Result<Rows, String>,
) -> Profile {
    let mut plan = Profiler::new(sources, options);
    while let Some(query) = plan.next_query() {
        match exec(&query.sql) {
            Ok(rows) => plan.feed(query.id, &rows),
            Err(reason) => plan.fail(query.id, reason),
        }
    }
    plan.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CSV file bound by its stem: `awards.csv` is `awards`.
    fn csv(key: &str) -> Source {
        Source {
            binding: key.trim_end_matches(".csv").into(),
            key: key.into(),
            location: key.into(),
            format: "csv".into(),
            option: None,
            etag: String::new(),
        }
    }

    fn rows(cells: &[&[&str]]) -> Rows {
        cells
            .iter()
            .map(|r| r.iter().map(|c| Some((*c).to_string())).collect())
            .collect()
    }

    #[test]
    fn a_source_is_read_with_its_rows_reader_and_the_option_its_binding_wrote() {
        let mut plan = Profiler::new(
            [Source {
                option: Some("|".into()),
                ..csv("o'brien.csv")
            }],
            Options::default(),
        );
        let q = plan.next_query().expect("a DESCRIBE");
        assert_eq!(
            q.sql,
            "DESCRIBE SELECT * FROM read_csv_auto('o''brien.csv', delim='|')"
        );
    }

    #[test]
    fn a_materialised_source_and_a_repeated_binding_are_not_read() {
        let mut plan = Profiler::new(
            [
                csv("a.csv"),
                csv("a.csv"),
                Source {
                    format: "rdf".into(),
                    ..csv("g.ttl")
                },
            ],
            Options::default(),
        );
        assert!(plan.next_query().is_some());
        assert!(
            plan.next_query().is_none(),
            "one DESCRIBE: the repeat and the graph are left out"
        );
    }

    #[test]
    fn two_bindings_of_one_file_are_two_sources_under_their_own_names() {
        let mut plan = Profiler::new(
            [
                csv("people.csv"),
                Source {
                    binding: "staff".into(),
                    ..csv("people.csv")
                },
            ],
            Options::TYPES,
        );
        while let Some(query) = plan.next_query() {
            plan.feed(query.id, &rows(&[&["id", "BIGINT"]]));
        }
        let profile = plan.finish();
        let named: Vec<(&str, &str)> = profile
            .sources
            .iter()
            .map(|s| (s.binding.as_str(), s.descriptor.key.as_str()))
            .collect();
        assert_eq!(named, [("people", "people.csv"), ("staff", "people.csv")]);
    }

    #[test]
    fn a_phase_waits_for_every_answer_before_the_next_begins() {
        let mut plan = Profiler::new([csv("a.csv"), csv("b.csv")], Options::default());
        let a = plan.next_query().expect("a's DESCRIBE");
        let b = plan.next_query().expect("b's DESCRIBE");
        plan.feed(a.id, &rows(&[&["id", "BIGINT"]]));
        assert!(plan.next_query().is_none(), "b is still out");
        plan.feed(b.id, &rows(&[&["id", "BIGINT"]]));
        assert!(
            plan.next_query()
                .expect("a count")
                .sql
                .starts_with("SELECT CAST(count(*)")
        );
    }

    /// The whole plan over two sources, answered by hand: what each count means.
    #[test]
    fn keys_and_inclusions_are_read_from_the_counts() {
        let mut plan = Profiler::new(
            [csv("awards.csv"), csv("laureates.csv")],
            Options {
                sample: 0,
                shown: 2,
                ..Options::default()
            },
        );
        let mut answers: HashMap<&str, Rows> = HashMap::from([
            (
                "DESCRIBE SELECT * FROM read_csv_auto('awards.csv')",
                rows(&[
                    &["id", "VARCHAR"],
                    &["laureate", "BIGINT"],
                    &["share", "DOUBLE"],
                ]),
            ),
            (
                "DESCRIBE SELECT * FROM read_csv_auto('laureates.csv')",
                rows(&[&["id", "BIGINT"], &["name", "VARCHAR"]]),
            ),
        ]);
        let mut asked = Vec::new();
        while let Some(q) = plan.next_query() {
            asked.push(q.sql.clone());
            let answer = if let Some(rows) = answers.remove(q.sql.as_str()) {
                rows
            } else if q.sql.contains("count(DISTINCT \"id\")") && q.sql.contains("awards.csv") {
                // 4 awards: ids unique, 3 laureates among them, one share repeated.
                rows(&[&[
                    "4",
                    "4",
                    "4",
                    "1903-physics-1",
                    "1954-chemistry-3",
                    "3",
                    "4",
                    "1",
                    "3",
                    "2",
                    "4",
                    "0.25",
                    "1.0",
                ]])
            } else if q.sql.contains("count(DISTINCT \"id\")") {
                // 3 laureates, ids and names unique.
                rows(&[&[
                    "3",
                    "3",
                    "3",
                    "1",
                    "3",
                    "3",
                    "3",
                    "Linus Pauling",
                    "Pierre Curie",
                ]])
            } else if q.sql.contains("LEFT JOIN")
                && q.sql.contains("CAST(\"laureate\"")
                && q.sql.contains("laureates.csv")
            {
                rows(&[&["3", "0"]])
            } else if q.sql.contains("LEFT JOIN") {
                rows(&[&["3", "2"]])
            } else if q.sql.contains("GROUP BY \"laureate\"") {
                rows(&[&["1", "2"]])
            } else if q.sql.contains("GROUP BY") {
                rows(&[&["1.0", "2"], &["0.25", "2"]])
            } else {
                rows(&[&["x", "1"]])
            };
            plan.feed(q.id, &answer);
        }
        assert!(plan.is_done());
        let profile = plan.finish();
        let awards = &profile.sources[0];
        assert_eq!(
            awards
                .descriptor
                .columns
                .iter()
                .map(|c| c.primitive)
                .collect::<Vec<_>>(),
            [Primitive::String, Primitive::Integer, Primitive::Float]
        );
        assert_eq!(
            awards.candidate_keys,
            ["id"],
            "laureate repeats; a float is never a key"
        );
        assert_eq!(profile.sources[1].candidate_keys, ["id", "name"]);
        let laureate = &awards.columns[1];
        assert_eq!(
            (laureate.min_value.as_deref(), laureate.max_value.as_deref()),
            (Some("1"), Some("3"))
        );
        assert_eq!(
            laureate.top_values,
            [TopValue {
                value: "1".into(),
                count: 2
            }],
            "a repeated value is one of the commonest"
        );
        assert!(
            awards.columns[0].top_values.is_empty(),
            "a key repeats nothing, and nothing was asked of it"
        );
        assert_eq!(awards.columns[2].top_values.len(), 2);
        assert_eq!(
            profile.inclusions,
            [Inclusion {
                from: ColumnRef {
                    source: "awards".into(),
                    column: "laureate".into()
                },
                to: ColumnRef {
                    source: "laureates".into(),
                    column: "id".into()
                },
            }],
        );
        // Only pairs of one primitive are tested: no float, and no string against an integer key.
        assert!(
            asked
                .iter()
                .filter(|q| q.contains("LEFT JOIN"))
                .all(|q| !q.contains("CAST(\"share\" AS VARCHAR) AS v"))
        );
        assert_eq!(awards.sample.len(), 1);
    }

    /// A value longer than 64 bytes is left out, never cut: a bound becomes
    /// absent, and a list of commonest values holding one goes whole.
    #[test]
    fn a_value_longer_than_64_bytes_is_left_out_and_never_cut() {
        let long = "x".repeat(MAX_VALUE_BYTES + 1);
        let mut plan = Profiler::new([csv("a.csv")], Options::default());
        let q = plan.next_query().expect("a DESCRIBE");
        plan.feed(q.id, &rows(&[&["note", "VARCHAR"]]));
        let q = plan.next_query().expect("the counts");
        plan.feed(q.id, &rows(&[&["4", "2", "4", "a", long.as_str()]]));
        let q = plan.next_query().expect("the commonest values");
        assert!(q.sql.contains("LIMIT 10"), "{}", q.sql);
        plan.feed(q.id, &rows(&[&[long.as_str(), "2"], &["a", "2"]]));
        while let Some(q) = plan.next_query() {
            plan.feed(q.id, &[]);
        }
        let note = &plan.finish().sources[0].columns[0];
        assert_eq!(note.min_value.as_deref(), Some("a"));
        assert_eq!(note.max_value, None);
        assert!(note.top_values.is_empty());
    }

    /// Tables the host holds are not described, and no inclusion is tested
    /// between them: the plan is the counts, and the keys they make.
    #[test]
    fn tables_are_counted_without_a_describe_or_an_inclusion() {
        let column = |name: &str, primitive| InferredColumn {
            name: name.into(),
            primitive,
        };
        let mut plan = Profiler::over_tables(
            [
                Table {
                    name: "Person".into(),
                    columns: vec![
                        column("dense_id", Primitive::Integer),
                        column("subject", Primitive::AnyUri),
                    ],
                },
                Table {
                    name: "Prize".into(),
                    columns: vec![
                        column("dense_id", Primitive::Integer),
                        column("subject", Primitive::AnyUri),
                        column("winner", Primitive::AnyUri),
                    ],
                },
            ],
            Options::WHOLE,
        );
        while let Some((id, ask)) = plan.next_ask() {
            assert!(
                matches!(ask, Ask::Counts { .. } | Ask::TopValues { .. }),
                "only counts and commonest values: {ask:?}"
            );
            if let Ask::Counts { columns, .. } = &ask {
                let mut row = vec!["2".to_string()];
                for _ in columns {
                    row.extend(["2", "2", "a", "b"].map(String::from));
                }
                plan.feed(id, &[row.into_iter().map(Some).collect()]);
            } else {
                plan.feed(id, &[]);
            }
        }
        let profile = plan.finish();
        assert!(profile.inclusions.is_empty());
        assert_eq!(profile.sources[0].candidate_keys, ["dense_id", "subject"]);
    }

    #[test]
    fn a_types_profile_is_one_describe_a_source_and_nothing_after() {
        let mut plan = Profiler::new([csv("a.csv")], Options::TYPES);
        let q = plan.next_query().expect("the DESCRIBE");
        plan.feed(q.id, &rows(&[&["id", "BIGINT"]]));
        assert!(
            plan.next_query().is_none(),
            "no counts, no inclusions, no sample"
        );
        assert!(plan.is_done());
        let profile = plan.finish();
        assert_eq!(profile.sources[0].descriptor.columns.len(), 1);
        assert_eq!(profile.sources[0].rows, None);
    }

    #[test]
    fn a_source_that_cannot_be_described_is_a_failure_and_not_a_profile() {
        let mut plan = Profiler::new([csv("gone.csv")], Options::default());
        let q = plan.next_query().expect("the DESCRIBE");
        plan.fail(q.id, "No files found that match the pattern");
        assert!(plan.next_query().is_none());
        assert!(plan.is_done());
        let profile = plan.finish();
        assert!(profile.sources.is_empty());
        assert_eq!(
            profile.failed,
            [Failure {
                source: "gone".into(),
                reason: "No files found that match the pattern".into()
            }]
        );
    }
}
