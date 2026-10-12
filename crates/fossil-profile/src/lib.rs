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
//! Every query casts what it selects to `VARCHAR`, so the two engines answer in
//! one form: rows of text cells, `None` for SQL `NULL`.
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

use fossil_base::{NativeReader, RowReader};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::Primitive;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

pub mod generated;

/// A query's answer: its rows, each cell the text of a value or `None` for `NULL`.
pub type Rows = Vec<Vec<Option<String>>>;

/// A source to profile, as its host can read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// What the program wrote — what the descriptor is keyed by.
    pub key: SmolStr,
    /// What the host's `DuckDB` reads: the location, or the name the host
    /// attached it under. Storage is the host's; this is only the name.
    pub name: String,
    /// The catalogue row its constructor names (`csv`), which picks the reader.
    pub format: String,
    /// The reader option the binding wrote (`delimiter = "|"`), verbatim.
    #[serde(default)]
    pub option: Option<String>,
    /// The host's validator for the bytes it read: carried into the descriptor.
    #[serde(default)]
    pub etag: String,
}

/// How much of a source a profile reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    /// The rows the counts read from each source; `0` reads every row.
    pub sample: u64,
    /// The rows kept as the profile's sample of each source.
    pub shown: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self { sample: 10_000, shown: 5 }
    }
}

/// One query for the host to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// What its rows are fed back with.
    pub id: u32,
    pub sql: String,
}

/// A column's counts, over the rows the profile read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnStats {
    pub name: SmolStr,
    pub distinct: u64,
    pub nulls: u64,
}

/// One source, profiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProfile {
    /// Its columns and their types: what the compiler's cache takes.
    pub descriptor: InferredDescriptor,
    /// The rows the counts read, and whether that was a sample.
    pub rows: Option<u64>,
    pub sampled: bool,
    pub columns: Vec<ColumnStats>,
    /// The columns that are candidate keys: never null, never repeated.
    pub keys: Vec<SmolStr>,
    /// Its first rows, every cell as text, in the descriptor's column order.
    pub sample: Rows,
}

/// A column of one source and a column of the same or another.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColumnRef {
    pub source: SmolStr,
    pub column: SmolStr,
}

/// Every value of `from` is a value of `to`, which is a candidate key: the
/// shape of an edge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Inclusion {
    pub from: ColumnRef,
    pub to: ColumnRef,
}

/// A query the host could not run, and what it said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// The source it was about: for an inclusion, the dependent side's.
    pub source: SmolStr,
    pub reason: String,
}

/// What the profiler found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    format!("{}('{}'{args})", reader.table_function(), name.replace('\'', "''"))
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

#[derive(Debug)]
struct State {
    source: Source,
    relation: String,
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
    Inclusion { from: (usize, usize), to: (usize, usize) },
    Sample(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Describe,
    Stats,
    Inclusions,
    Sample,
    Done,
}

/// The plan: describe every source, count each one's columns, test the
/// inclusions the types allow, keep a sample. A phase's queries are handed out
/// together, so a host may run them at once; the next phase begins once all of
/// them are answered.
#[derive(Debug)]
pub struct Profiler {
    options: Options,
    states: Vec<State>,
    phase: Phase,
    queue: VecDeque<Query>,
    pending: HashMap<u32, Pending>,
    next_id: u32,
    inclusions: Vec<Inclusion>,
    failed: Vec<Failure>,
}

impl Profiler {
    /// A plan over `sources`. One per key: a second source with a key already
    /// given is the same file and is not read again. A source whose format reads
    /// through no native function is left out — there is nothing to describe.
    #[must_use]
    pub fn new(sources: impl IntoIterator<Item = Source>, options: Options) -> Self {
        let mut states: Vec<State> = Vec::new();
        for source in sources {
            let Some(reader) = native_reader(&source.format) else { continue };
            if states.iter().any(|s| s.source.key == source.key) {
                continue;
            }
            let relation = relation(reader, &source.name, source.option.as_deref());
            states.push(State {
                source,
                relation,
                described: false,
                columns: Vec::new(),
                rows: None,
                stats: Vec::new(),
                sample: Vec::new(),
            });
        }
        let mut plan = Self {
            options,
            states,
            phase: Phase::Describe,
            queue: VecDeque::new(),
            pending: HashMap::new(),
            next_id: 0,
            inclusions: Vec::new(),
            failed: Vec::new(),
        };
        for i in 0..plan.states.len() {
            let sql = format!("DESCRIBE SELECT * FROM {}", plan.states[i].relation);
            plan.ask(sql, Pending::Describe(i));
        }
        plan
    }

    fn ask(&mut self, sql: String, what: Pending) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, what);
        self.queue.push_back(Query { id, sql });
    }

    /// What the counts read from source `i`: a sample of it, or all of it.
    fn sampled(&self, i: usize) -> String {
        let relation = &self.states[i].relation;
        match self.options.sample {
            0 => format!("(SELECT * FROM {relation})"),
            n => format!("(SELECT * FROM {relation} LIMIT {n})"),
        }
    }

    /// The next query to run, or `None` when there is none to hand out now:
    /// either the profile is done, or the phase waits on answers still out.
    pub fn next_query(&mut self) -> Option<Query> {
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
            Phase::Describe => {
                for i in 0..self.states.len() {
                    let columns = &self.states[i].columns;
                    if columns.is_empty() {
                        continue;
                    }
                    let mut select = vec!["CAST(count(*) AS VARCHAR)".to_string()];
                    for c in columns {
                        let name = ident(&c.name);
                        select.push(format!("CAST(count(DISTINCT {name}) AS VARCHAR)"));
                        select.push(format!("CAST(count({name}) AS VARCHAR)"));
                    }
                    let sql = format!("SELECT {} FROM {} AS s", select.join(", "), self.sampled(i));
                    self.ask(sql, Pending::Stats(i));
                }
                Phase::Stats
            }
            Phase::Stats => {
                self.plan_inclusions();
                Phase::Inclusions
            }
            Phase::Inclusions => {
                for i in 0..self.states.len() {
                    let columns = &self.states[i].columns;
                    if columns.is_empty() || self.options.shown == 0 {
                        continue;
                    }
                    let select: Vec<String> = columns.iter().map(|c| text(&c.name)).collect();
                    let sql = format!(
                        "SELECT {} FROM {} LIMIT {}",
                        select.join(", "),
                        self.states[i].relation,
                        self.options.shown
                    );
                    self.ask(sql, Pending::Sample(i));
                }
                Phase::Sample
            }
            Phase::Sample | Phase::Done => Phase::Done,
        };
    }

    /// Every pair a type allows: a column with values, and a candidate key of
    /// the same primitive that is not the column itself.
    fn plan_inclusions(&mut self) {
        let mut asks = Vec::new();
        for (b, target) in self.states.iter().enumerate() {
            for key in &self.keys(b) {
                let Some(k) = target.columns.iter().position(|c| &c.name == key) else { continue };
                let kind = target.columns[k].primitive;
                for (a, dependent) in self.states.iter().enumerate() {
                    for (c, column) in dependent.columns.iter().enumerate() {
                        let has_values = dependent
                            .stats
                            .get(c)
                            .is_some_and(|s| dependent.rows.is_some_and(|rows| s.nulls < rows));
                        if (a, c) == (b, k) || column.primitive != kind || !has_values {
                            continue;
                        }
                        let sql = format!(
                            "SELECT CAST(count(*) AS VARCHAR), CAST(count(*) FILTER (WHERE y.v IS NULL) AS VARCHAR) \
                             FROM (SELECT DISTINCT {from} AS v FROM {dependent} AS a WHERE {col} IS NOT NULL) AS x \
                             LEFT JOIN (SELECT DISTINCT {to} AS v FROM {referenced} AS b) AS y ON x.v = y.v",
                            from = text(&column.name),
                            col = ident(&column.name),
                            dependent = self.sampled(a),
                            to = text(key),
                            referenced = format!("(SELECT * FROM {})", target.relation),
                        );
                        asks.push((sql, Pending::Inclusion { from: (a, c), to: (b, k) }));
                    }
                }
            }
        }
        for (sql, what) in asks {
            self.ask(sql, what);
        }
    }

    /// Source `i`'s candidate keys: columns a key can be, never null and never
    /// repeated in the rows read — and there were rows.
    fn keys(&self, i: usize) -> Vec<SmolStr> {
        let state = &self.states[i];
        let Some(rows) = state.rows.filter(|&r| r > 0) else { return Vec::new() };
        state
            .columns
            .iter()
            .zip(&state.stats)
            .filter(|(c, s)| keyable(c.primitive) && s.nulls == 0 && s.distinct == rows)
            .map(|(c, _)| c.name.clone())
            .collect()
    }

    /// The rows query `id` answered.
    pub fn feed(&mut self, id: u32, rows: &[Vec<Option<String>>]) {
        let Some(what) = self.pending.remove(&id) else { return };
        let count = |cell: Option<&Option<String>>| cell.and_then(|c| c.as_deref()).and_then(|c| c.parse::<u64>().ok());
        match what {
            Pending::Describe(i) => {
                let reader = native_reader(&self.states[i].source.format);
                let state = &mut self.states[i];
                state.described = true;
                state.columns = rows
                    .iter()
                    .filter_map(|row| {
                        let name = row.first()?.as_deref()?;
                        let kind = row.get(1)?.as_deref()?;
                        let primitive = reader.map_or(Primitive::String, |r| generated::read_primitive(r, kind));
                        (!name.is_empty()).then(|| InferredColumn { name: name.into(), primitive })
                    })
                    .collect();
            }
            Pending::Stats(i) => {
                let Some(row) = rows.first() else { return };
                let state = &mut self.states[i];
                let Some(total) = count(row.first()) else { return };
                state.rows = Some(total);
                state.stats = state
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(n, c)| {
                        let distinct = count(row.get(1 + 2 * n)).unwrap_or(0);
                        let present = count(row.get(2 + 2 * n)).unwrap_or(0);
                        ColumnStats { name: c.name.clone(), distinct, nulls: total.saturating_sub(present) }
                    })
                    .collect();
            }
            Pending::Inclusion { from, to } => {
                let Some(row) = rows.first() else { return };
                let (Some(values), Some(missing)) = (count(row.first()), count(row.get(1))) else { return };
                if values > 0 && missing == 0 {
                    let at = |(s, c): (usize, usize)| ColumnRef {
                        source: self.states[s].source.key.clone(),
                        column: self.states[s].columns[c].name.clone(),
                    };
                    self.inclusions.push(Inclusion { from: at(from), to: at(to) });
                }
            }
            Pending::Sample(i) => self.states[i].sample = rows.to_vec(),
        }
    }

    /// Query `id` could not be run, and `reason` is what the host said.
    pub fn fail(&mut self, id: u32, reason: impl Into<String>) {
        let Some(what) = self.pending.remove(&id) else { return };
        let i = match what {
            Pending::Describe(i) | Pending::Stats(i) | Pending::Sample(i) => i,
            Pending::Inclusion { from: (a, _), .. } => a,
        };
        self.failed.push(Failure { source: self.states[i].source.key.clone(), reason: reason.into() });
    }

    /// The profile. Called before [`Profiler::is_done`], it holds what was
    /// answered so far.
    #[must_use]
    pub fn finish(self) -> Profile {
        let keys: Vec<Vec<SmolStr>> = (0..self.states.len()).map(|i| self.keys(i)).collect();
        let sampled = self.options.sample;
        let sources = self
            .states
            .into_iter()
            .zip(keys)
            .filter(|(s, _)| s.described)
            .map(|(s, keys)| SourceProfile {
                sampled: sampled > 0 && s.rows.is_some_and(|r| r >= sampled),
                rows: s.rows,
                columns: s.stats,
                keys,
                sample: s.sample,
                descriptor: InferredDescriptor { key: s.source.key, columns: s.columns, etag: s.source.etag },
            })
            .collect();
        Profile { sources, inclusions: self.inclusions, failed: self.failed }
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

    fn csv(key: &str) -> Source {
        Source { key: key.into(), name: key.into(), format: "csv".into(), option: None, etag: String::new() }
    }

    fn rows(cells: &[&[&str]]) -> Rows {
        cells.iter().map(|r| r.iter().map(|c| Some((*c).to_string())).collect()).collect()
    }

    #[test]
    fn a_source_is_read_with_its_rows_reader_and_the_option_its_binding_wrote() {
        let mut plan = Profiler::new(
            [Source { option: Some("|".into()), ..csv("o'brien.csv") }],
            Options::default(),
        );
        let q = plan.next_query().expect("a DESCRIBE");
        assert_eq!(q.sql, "DESCRIBE SELECT * FROM read_csv_auto('o''brien.csv', delim='|')");
    }

    #[test]
    fn a_materialised_source_and_a_repeated_key_are_not_read() {
        let mut plan = Profiler::new(
            [csv("a.csv"), csv("a.csv"), Source { format: "rdf".into(), ..csv("g.ttl") }],
            Options::default(),
        );
        assert!(plan.next_query().is_some());
        assert!(plan.next_query().is_none(), "one DESCRIBE: the repeat and the graph are left out");
    }

    #[test]
    fn a_phase_waits_for_every_answer_before_the_next_begins() {
        let mut plan = Profiler::new([csv("a.csv"), csv("b.csv")], Options::default());
        let a = plan.next_query().expect("a's DESCRIBE");
        let b = plan.next_query().expect("b's DESCRIBE");
        plan.feed(a.id, &rows(&[&["id", "BIGINT"]]));
        assert!(plan.next_query().is_none(), "b is still out");
        plan.feed(b.id, &rows(&[&["id", "BIGINT"]]));
        assert!(plan.next_query().expect("a count").sql.starts_with("SELECT CAST(count(*)"));
    }

    /// The whole plan over two sources, answered by hand: what each count means.
    #[test]
    fn keys_and_inclusions_are_read_from_the_counts() {
        let mut plan = Profiler::new([csv("awards.csv"), csv("laureates.csv")], Options { sample: 0, shown: 2 });
        let mut answers: HashMap<&str, Rows> = HashMap::from([
            ("DESCRIBE SELECT * FROM read_csv_auto('awards.csv')", rows(&[&["id", "VARCHAR"], &["laureate", "BIGINT"], &["share", "DOUBLE"]])),
            ("DESCRIBE SELECT * FROM read_csv_auto('laureates.csv')", rows(&[&["id", "BIGINT"], &["name", "VARCHAR"]])),
        ]);
        let mut asked = Vec::new();
        while let Some(q) = plan.next_query() {
            asked.push(q.sql.clone());
            let answer = if let Some(rows) = answers.remove(q.sql.as_str()) {
                rows
            } else if q.sql.contains("count(DISTINCT \"id\")") && q.sql.contains("awards.csv") {
                // 4 awards: ids unique, 3 laureates among them, one share repeated.
                rows(&[&["4", "4", "4", "3", "4", "2", "4"]])
            } else if q.sql.contains("count(DISTINCT \"id\")") {
                // 3 laureates, ids and names unique.
                rows(&[&["3", "3", "3", "3", "3"]])
            } else if q.sql.contains("LEFT JOIN") && q.sql.contains("CAST(\"laureate\"") && q.sql.contains("laureates.csv") {
                rows(&[&["3", "0"]])
            } else if q.sql.contains("LEFT JOIN") {
                rows(&[&["3", "2"]])
            } else {
                rows(&[&["x", "1"]])
            };
            plan.feed(q.id, &answer);
        }
        assert!(plan.is_done());
        let profile = plan.finish();
        let awards = &profile.sources[0];
        assert_eq!(awards.descriptor.columns.iter().map(|c| c.primitive).collect::<Vec<_>>(), [Primitive::String, Primitive::Integer, Primitive::Float]);
        assert_eq!(awards.keys, ["id"], "laureate repeats; a float is never a key");
        assert_eq!(profile.sources[1].keys, ["id", "name"]);
        assert_eq!(
            profile.inclusions,
            [Inclusion {
                from: ColumnRef { source: "awards.csv".into(), column: "laureate".into() },
                to: ColumnRef { source: "laureates.csv".into(), column: "id".into() },
            }],
        );
        // Only pairs of one primitive are tested: no float, and no string against an integer key.
        assert!(asked.iter().all(|q| !q.contains("CAST(\"share\" AS VARCHAR) AS v")));
        assert_eq!(awards.sample.len(), 1);
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
        assert_eq!(profile.failed, [Failure { source: "gone.csv".into(), reason: "No files found that match the pattern".into() }]);
    }
}
