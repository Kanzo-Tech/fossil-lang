//! `fossil-introspect` — what a NATIVE host does before it asks the compiler to
//! compile: read each source's columns, and hold the credentials that let it.
//!
//! # Why this is a crate and not a module of the native host
//!
//! Because the compiler does not introspect. **The host does**, and one host
//! already proved it: `fossil-wasm` implements
//! [`fossil_base::System::descriptors`], the browser runs
//! `@fossil-lang/introspect` against its own DuckDB-WASM, and the registered
//! descriptors are all the checker ever sees. The native side did the same job
//! from *inside* the native host, which is what made that crate open a `DuckDB`
//! connection and carry a `compile_error!` for `wasm32`.
//!
//! Moving it does not change what the compiler infers, and that is the point of
//! this shape rather than a `DataFusion` rewrite: **`DuckDB` is still the engine
//! that DESCRIBES, on both sides.** `packages/introspect/tests/rust-parity.test.ts`
//! compares this crate against the TypeScript for exactly that reason — «the
//! browser and the CLI answer the same program differently» is the bug it
//! exists to catch, and CSV type sniffing is where two engines would disagree.
//!
//! # Credentials came too, and they had to
//!
//! [`creds`] holds `RunCreds` — the `--creds-stdin` payload. It names
//! `fossil-resolver`, which carries its OWN `wasm32` tripwire because cloud
//! credentials must not cross the wasm boundary. So a native host that
//! still parsed credentials could not be wasm-clean no matter what happened to
//! `DuckDB`. Introspection and credentials are the same concern anyway: the
//! secret exists so that the `DESCRIBE` over a cloud `@conn` source
//! authenticates.
//!
//! The host therefore takes a `HashMap<String, String>` of connection
//! URLs and never sees a secret.
//!
//! # What this crate is NOT
//!
//! It is not a second compiler entry point. It fills a cache and returns
//! nothing; `fossil_cli::check` / `run` read that cache through `System`. The
//! order — introspect, then compile — is the caller's, and it is the order the
//! browser has always used.

use std::collections::HashMap;

use fossil_base::System;
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::Primitive;
use fossil_locator::SourceAnchor;
use smol_str::SmolStr;

pub mod creds;

pub use creds::{ConnectionCreds, RunCreds, SecretSpec};

/// How far a host is willing to reach for a source's columns.
///
/// It is a question about **who is waiting**, not about the source. There are
/// two hosts on this side of the seam and they answer it differently, so the
/// answer is a parameter rather than a rule inside the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Every source the program names, including a locator whose `DESCRIBE` is
    /// a network round trip. What a COMMAND does: `fossil check` and
    /// `fossil run` are the thing the user is already waiting for, and a source
    /// they cannot read is a source the answer would be wrong without.
    Anywhere,
    /// Only a source this host can `stat`: a local file that exists right now.
    ///
    /// What an EDITOR does. `fossil-lsp`'s `main_loop` is one sequential loop
    /// over one channel, so a `didOpen` blocked on `s3://` is not one slow
    /// file — it is hover and completion dead in every other buffer for as long
    /// as the read takes. An editor may go and read a CSV beside the program; it
    /// may not go on the network.
    ///
    /// The discriminator is [`freshness_token`] and not a scheme test, because
    /// the two questions have one answer: a locator this host cannot `stat` is
    /// exactly a locator whose freshness it cannot establish, which is already
    /// the empty token. It also covers what a scheme test would miss — a
    /// relative path that does not exist yet, and the half-typed one a keystroke
    /// produces on the way to it.
    Local,
}

/// Map a `DuckDB` column-type string onto the lattice — the native sibling of
/// `@fossil-lang/introspect`'s `duckdbTypeToFossilPrimitive`. A vocabulary the
/// engine reads and nobody else does, which is why it lives here and not on
/// [`Primitive`]; the xsd direction is the one the lattice owns.
fn duckdb_type_to_fossil_primitive(t: &str) -> Primitive {
    let upper = t.trim().to_ascii_uppercase();
    match upper.as_str() {
        "INTEGER" | "BIGINT" | "INT" | "SMALLINT" | "TINYINT" | "HUGEINT" => Primitive::Integer,
        "DOUBLE" | "FLOAT" | "REAL" => Primitive::Float,
        t if t.starts_with("DECIMAL") => Primitive::Float,
        "BOOLEAN" | "BOOL" => Primitive::Bool,
        "DATE" => Primitive::Date,
        // Every spelling DuckDB has for an instant, because the arm was a list
        // of two and DuckDB has seven. `TIMESTAMP WITH TIME ZONE` is what
        // `read_csv_auto` infers for an ISO-8601 string carrying an offset —
        // which is how LDBC-SNB dates every row it ships — and it fell to the
        // `_` arm and came back `String`. The consequence is not a slow path:
        // the checker compares this against the shape's declared `Primitive`,
        // so `xsd:dateTime` over such a column is a hard type error and the
        // column is unwritable as anything but `xsd:string`.
        t if t.starts_with("TIMESTAMP") => Primitive::DateTime,
        "DATETIME" => Primitive::DateTime,
        t if t.starts_with("TIME") => Primitive::Time,
        _ => Primitive::String,
    }
}

/// The constructors this scraper looks for: the rows `catalogue.bnf` gives a
/// `reads native <fn>`.
///
/// **Introspection is a `DESCRIBE` through a table function**, so a row that
/// reads `materialised` — `io.rdf` — has nothing to describe it with and is
/// correctly absent. That used to be an alternation of three literals which
/// happened to be the same three; now the reason is the selection.
fn native_rows() -> impl Iterator<Item = &'static fossil_base::Provider> {
    fossil_base::providers::DATA
        .iter()
        .copied()
        .filter(|p| matches!(p.reads_rows, Some(fossil_base::RowReader::Native(_))))
}

/// One source binding, as the scrape sees it.
struct ScrapedRef {
    /// The bound name — `User`.
    source_name: SmolStr,
    /// The row written after `io.` — `csv`.
    constructor: SmolStr,
    /// The positional URI, as the program wrote it.
    raw_uri: String,
    /// The reader option, if the binding named one — `io.csv("u.csv",
    /// delimiter = "|")` → `|`.
    ///
    /// **The introspecting DESCRIBE has to carry it or it describes a different
    /// file than the run reads.** A pipe-delimited CSV read with a comma is one
    /// column called `id|name|birthday`, so an introspection that dropped the
    /// option would type the source's columns out of a schema the executor
    /// never produces — and the mapping would be refused for naming columns
    /// that are, in fact, there.
    option: Option<String>,
}

/// Scrape source-binding RHS source URLs from a `.fossil` file's text. It is a
/// regex placeholder for an AST walk, and it is wrong on any binding the regex
/// cannot see. The browser no longer scrapes: `@fossil-lang/introspect` takes
/// its sources from the AST, through `FossilPlayground.sources`.
///
/// **The alternation is not written here**: it is built from the catalogue, so
/// a constructor added to `catalogue.bnf` reaches this scrape at once.
///
/// **The reader option is the second interpolation and arrives the same way.**
/// The word a program writes for it (`delimiter`) is
/// `fossil_base::Provider::options`, generated from the same rows, so this
/// pattern does not spell it. The alternation is over every
/// option any native row declares — one today — because the scrape runs before
/// the constructor is known and `fossil_hir::lower::check_reader_option` is
/// what refuses an option written on the wrong row.
fn extract_source_refs(text: &str) -> Vec<ScrapedRef> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        let alternation = native_rows().map(|p| p.name).collect::<Vec<_>>().join("|");
        let options = native_rows()
            .flat_map(|p| p.options.iter().copied())
            .collect::<Vec<_>>()
            .join("|");
        regex::Regex::new(&format!(
            r#"(\w[\w\d_]*)\s*:=\s*io\.({alternation})\(\s*['"]([^'"]+)['"](?:\s*,\s*(?:{options})\s*=\s*['"]([^'"]*)['"])?"#
        ))
        .expect("the catalogue's constructor and option names are regex-safe")
    });
    re.captures_iter(text)
        .map(|c| ScrapedRef {
            source_name: SmolStr::from(c.get(1).unwrap().as_str()),
            constructor: SmolStr::from(c.get(2).unwrap().as_str()),
            raw_uri: c.get(3).unwrap().as_str().to_string(),
            option: c.get(4).map(|m| m.as_str().to_string()),
        })
        .collect()
}

/// The `DuckDB` named parameter a native reader's option is spelled with.
///
/// **This is the ENGINE's vocabulary and belongs here**, beside the SQL it goes
/// into — exactly as `read_csv_auto` is the catalogue's word and
/// `CsvReadOptions::delimiter` is `DataFusion`'s. `catalogue.bnf` names the
/// position the PROGRAM writes (`delimiter`), and the two engines spell it
/// differently enough that no one token could serve both: `delim=` is a SQL
/// named argument and the other is a Rust method taking a byte.
///
/// Exhaustive, so a new native reader is a compile error here until somebody
/// decides whether it has options and what `DuckDB` calls them.
const fn duckdb_option_keyword(r: fossil_base::NativeReader) -> Option<&'static str> {
    match r {
        fossil_base::NativeReader::CsvAuto => Some("delim"),
        fossil_base::NativeReader::JsonAuto | fossil_base::NativeReader::Parquet => None,
    }
}

/// The token that decides whether a cached descriptor still describes its
/// source: the file's modification time in nanoseconds since the epoch, paired
/// with its byte length. Two `stat` fields, no read of the source itself: the
/// native host does NOT hash the bytes because hashing means reading the whole
/// source to decide whether the source needs reading — hundreds of megabytes
/// to save a `DESCRIBE` that reads the first rows, which would make the cache
/// cost more than the thing it caches. `mtime` can say "changed" when nothing
/// did (a `touch`), which costs one extra `DESCRIBE` and no wrong answer;
/// pairing it with the size is what narrows the one case that *is* wrong, a
/// file restored with both its old `mtime` and its exact old length.
///
/// Returns `""` for anything this host cannot `stat` — an `http(s)://` or
/// `s3://` locator, or a path that does not exist. An empty token is never
/// fresh ([`fossil_descriptors_input::DescriptorCache::is_fresh`]), so those are
/// re-introspected on every compile. That is the honest answer for an object we
/// would have to make a network round trip to interrogate.
fn freshness_token(resolved: &str) -> String {
    let Ok(meta) = std::fs::metadata(resolved) else {
        return String::new();
    };
    let Ok(modified) = meta.modified() else {
        return String::new();
    };
    let Ok(since_epoch) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) else {
        return String::new();
    };
    format!(
        "mtime:{}.{:09}:size:{}",
        since_epoch.as_secs(),
        since_epoch.subsec_nanos(),
        meta.len()
    )
}

/// Pre-introspect every source the program names — [`Reach::Anywhere`],
/// because a host that reads the program off the disk is a COMMAND, and a
/// command is allowed to wait. The editor does not come through here: its copy
/// of the program is a buffer that may never have been saved, so it calls
/// [`pre_introspect_and_register`] with the text it already holds.
///
/// Reads the file at `path`, anchors it, and registers an
/// [`InferredDescriptor`] for every source it binds on `system`'s cache BEFORE
/// typecheck — keyed by the URI the program writes rather than by the resolved
/// locator, which is the only string the host and the checker both see.
///
/// **This is the unit of work a host actually has**, and the reason it is here
/// rather than repeated at each call site: `pre_introspect_and_register` takes
/// text that has already been read and an anchor that has already been built,
/// and every caller wanted the same three lines in front of
/// it. The `System` is the caller's because only the caller knows which host it
/// is — `fossil_cli::host_system(path)` natively, and the browser does not
/// come through here at all.
///
/// # Errors
///
/// If `path` cannot be read. Per-source introspection failures are NOT errors:
/// they log and skip, so a compile can still succeed with no forward-propagated
/// types for that source.
#[allow(clippy::implicit_hasher)] // the host builds one map and passes it; a
// generic hasher here would be a parameter no caller varies.
pub fn introspect_program(
    system: &dyn System,
    path: &std::path::Path,
    connections: &HashMap<String, String>,
    creds: &RunCreds,
) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let program_dir = fossil_locator::program_dir(&path.to_string_lossy());
    let anchor = SourceAnchor::new(&program_dir, connections);
    pre_introspect_and_register(system, &text, anchor, &creds.connections, Reach::Anywhere);
    Ok(())
}

/// A source whose cached descriptor still carries the current
/// `freshness_token` is skipped — no `DESCRIBE`, no read. That is where the
/// cost is: programs are small and sources are not, so the introspection is
/// the expensive half of a compile and it is the half that rarely needs doing
/// twice.
///
/// Per-source failures are non-fatal — they log + skip; the compile may still
/// succeed with no forward propagation for that source.
///
/// `reach` is the caller's answer to "may this block on the network?" — see
/// [`Reach`]. It is a parameter and not a property of the source because the
/// same `s3://` URI is a legitimate read for `fossil check` and a stalled
/// editor for `fossil-lsp`.
#[allow(clippy::implicit_hasher)] // as `introspect_program`.
pub fn pre_introspect_and_register(
    system: &dyn System,
    source_text: &str,
    anchor: SourceAnchor<'_>,
    connections: &HashMap<String, ConnectionCreds>,
    reach: Reach,
) {
    let Some(cache) = system.descriptors() else {
        tracing::debug!("host keeps no descriptor cache; skipping pre-introspection");
        return;
    };

    // Opened on the first miss, not on entry. A compile whose sources are all
    // fresh must do no DuckDB work at all, and opening a connection is work.
    let mut conn: Option<duckdb::Connection> = None;

    for ScrapedRef {
        source_name,
        constructor,
        raw_uri,
        option,
    } in extract_source_refs(source_text)
    {
        let token = freshness_token(&anchor.locator(&raw_uri));
        // An empty token means this host could not `stat` the locator — a
        // scheme it does not own, or a path that is not there. Under
        // `Reach::Local` that is the whole filter, and it is deliberately
        // checked BEFORE `is_fresh`: an empty token is never fresh, so without
        // this the editor would open a connection and attempt the read on every
        // single keystroke.
        if reach == Reach::Local && token.is_empty() {
            tracing::debug!(
                "`{raw_uri}` is not a file this host can stat; not reading it from here"
            );
            continue;
        }
        if cache.is_fresh(&raw_uri, &token) {
            tracing::debug!("`{raw_uri}` is unchanged since it was introspected; reusing");
            continue;
        }

        let conn = if let Some(c) = &conn {
            c
        } else {
            let opened = match duckdb::Connection::open_in_memory() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("DuckDB in-memory open failed; skipping pre-introspection: {e}");
                    return;
                }
            };
            if let Err(e) = apply_source_creds(&opened, connections) {
                tracing::warn!("applying source creds for pre-introspection failed: {e}");
            }
            conn.insert(opened)
        };

        // Resolved a second time deliberately: the token above is about the
        // bytes on disk, this is the string DuckDB reads, and conflating them
        // would make a `@conn` alias silently change meaning between the two.
        let resolved_path = anchor.locator(&raw_uri);
        let escaped_path = resolved_path.replace('\'', "''");
        // The CONSTRUCTOR chooses the reader, and it used to not: every source
        // was `read_csv_auto` whatever `io.` said. A JSON array read as CSV
        // introspects to one column named after the first line, so
        // `data/sightings.json` — which opens with a bare `[` — produced a
        // schema whose only column was literally `[`, and every real column
        // came back as `unknown column \`id\` — did you mean \`[\`?`. The
        // did-you-mean is what made it legible: it printed the wrong schema.
        //
        // The three arms were a second copy of `catalogue.bnf`'s `native <fn>`
        // tokens, and the `_ =>` fallback was the ORIGINAL BUG wearing a
        // default: unreachable only for as long as the alternation above listed
        // exactly the constructors this match named. Both are the catalogue's
        // answer now, and a row the table does not know is skipped loudly
        // rather than read as CSV.
        let Some(native) = native_rows()
            .find(|p| p.name == constructor.as_str())
            .and_then(|p| match p.reads_rows {
                Some(fossil_base::RowReader::Native(r)) => Some(r),
                _ => None,
            })
        else {
            tracing::warn!(
                "source `{source_name}` names `io.{constructor}`, which is not a \
                 natively-readable catalogue row; skipping pre-introspection"
            );
            continue;
        };
        let reader = native.table_function();
        // **The option the binding wrote, in DuckDB's spelling, or nothing.**
        // Nothing is the whole of the absent case: `read_csv_auto` sniffs, and
        // substituting a comma here would make this DESCRIBE describe a file
        // the executor does not read — the divergence naming a delimiter exists
        // to close. `fossil_hir::lower::check_reader_option` has already refused
        // a value that is not one character and one written on a row that takes
        // none, so what arrives here is either absent or usable.
        let args = match (option.as_deref(), duckdb_option_keyword(native)) {
            (Some(value), Some(keyword)) => {
                format!(", {keyword}='{}'", value.replace('\'', "''"))
            }
            _ => String::new(),
        };
        let sql = format!("DESCRIBE SELECT * FROM {reader}('{escaped_path}'{args})");
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "DESCRIBE prepare failed for source `{source_name}` (uri=`{raw_uri}`): {e}"
                );
                continue;
            }
        };
        let cols: Vec<InferredColumn> = match stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let typ: String = row.get(1)?;
            Ok(InferredColumn {
                name: SmolStr::from(name),
                primitive: duckdb_type_to_fossil_primitive(&typ),
            })
        }) {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!("DESCRIBE query_map failed for `{source_name}`: {e}");
                continue;
            }
        };
        cache.insert(InferredDescriptor {
            uri: SmolStr::from(raw_uri.as_str()),
            columns: cols,
            freshness_token: token,
        });
        tracing::debug!("introspected `{raw_uri}` for source `{source_name}`");
    }
}

/// The name→base-URL view of the run's connections — what
/// [`fossil_locator::SourceAnchor`] expands a `@conn` alias through, and what the
/// executor is handed for the same purpose.
///
/// Projected ONCE per command and then borrowed, rather than rebuilt inside a
/// per-URI resolver: the map was cloned for every source of every program, and
/// a second copy of it was built again at the executor seam. One projection is
/// also what lets the anchor be a borrow — the pair (directory, connections)
/// has to outlive every resolution done against it, which is exactly the
/// lifetime of the command.
#[allow(clippy::implicit_hasher)] // as `introspect_program`.
pub fn connection_urls(connections: &HashMap<String, ConnectionCreds>) -> HashMap<String, String> {
    connections
        .iter()
        .map(|(name, c)| (name.clone(), c.url.clone()))
        .collect()
}

/// Install each source connection's scoped read secret on `conn`, so a
/// `read_csv_auto` over a cloud `@conn` source authenticates. No-op for
/// connections without a secret (local / public-URL sources).
///
/// This ran through `fossil_layout::install_secret`, and that indirection is
/// gone. The rendering — which is the part with a decision in it, and the part
/// with tests — is [`fossil_resolver::ResolvedPath::create_secret_sql`], in `fossil-resolver`,
/// and it has not moved. What wrapped it was `conn.execute_batch(sql)` under an
/// error enum that both of its two callers immediately flattened to a string.
/// A crate does not need a dependency to run one statement on a connection it
/// already holds, and that dependency was the last thing making `fossil-layout`
/// link `DuckDB`.
#[allow(clippy::implicit_hasher)] // as `introspect_program`.
pub fn apply_source_creds(
    conn: &duckdb::Connection,
    connections: &HashMap<String, ConnectionCreds>,
) -> miette::Result<()> {
    for (i, c) in connections.values().enumerate() {
        if let Some(spec) = &c.secret {
            let resolved =
                fossil_resolver::ResolvedPath::with_secret(&c.url, spec.to_cloud_secret());
            if let Some(sql) = resolved.create_secret_sql(&format!("__fossil_src_{i}")) {
                conn.execute_batch(&sql)
                    .map_err(|e| miette::miette!("install source secret: {e}"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_base::test_support::NativeSystem;

    fn conns(pairs: &[(&str, &str)]) -> HashMap<String, ConnectionCreds> {
        pairs
            .iter()
            .map(|(name, url)| {
                (
                    (*name).to_string(),
                    ConnectionCreds {
                        url: (*url).to_string(),
                        secret: None,
                    },
                )
            })
            .collect()
    }

    /// Every spelling `DuckDB` has for an instant maps to an instant.
    ///
    /// **The table is derived from `DuckDB` and not from memory**: the arms are
    /// asserted against the `DESCRIBE` of a value `DuckDB` itself typed, so a
    /// version that renames a type fails here rather than silently widening a
    /// column to `String`. That widening is what this test exists for — the arm
    /// was `"TIMESTAMP" | "DATETIME"`, `read_csv_auto` infers `TIMESTAMP WITH
    /// TIME ZONE` for any ISO-8601 string carrying an offset, and the checker
    /// compares this answer against the shape's declared `Primitive`. So the
    /// gap was not a slow path: it made `xsd:dateTime` a hard type error over
    /// every offset-bearing column, which is how LDBC-SNB dates every row.
    #[test]
    fn every_duckdb_instant_is_an_instant() {
        for (sql, expected) in [
            ("TIMESTAMP '2012-01-01 00:00:00'", Primitive::DateTime),
            ("TIMESTAMPTZ '2012-01-01 00:00:00+01'", Primitive::DateTime),
            (
                "CAST('2012-01-01T00:00:00.000+00:00' AS TIMESTAMP WITH TIME ZONE)",
                Primitive::DateTime,
            ),
            ("DATE '2012-01-01'", Primitive::Date),
            ("TIME '12:00:00'", Primitive::Time),
            ("CAST(1 AS BIGINT)", Primitive::Integer),
            ("CAST(1.5 AS DOUBLE)", Primitive::Float),
            ("CAST(1.5 AS DECIMAL(38,18))", Primitive::Float),
            ("true", Primitive::Bool),
            ("'x'", Primitive::String),
        ] {
            let conn = duckdb::Connection::open_in_memory().expect("in-memory duckdb");
            let mut stmt = conn
                .prepare(&format!("DESCRIBE SELECT {sql} AS v"))
                .expect("describe prepares");
            let declared: String = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .expect("describe runs")
                .next()
                .expect("one column")
                .expect("one row");
            assert_eq!(
                duckdb_type_to_fossil_primitive(&declared),
                expected,
                "DuckDB types `{sql}` as `{declared}`, which must not widen to String"
            );
        }
    }

    /// **The scrape reads the reader option, and reads it only where the
    /// program wrote one.**
    ///
    /// The word it looks for is `fossil_base::Provider::options`, generated
    /// from `catalogue.bnf`, so this asserts the SCAFFOLD around it and not the
    /// word: the URI stays positional, the option is optional, and a binding
    /// with neither reads exactly as it always did.
    #[test]
    fn the_scrape_reads_the_option_a_binding_wrote() {
        let text = "\
A := io.csv(\"a.csv\", delimiter = \"|\")
B := io.csv(\"b.csv\")
C := io.parquet(\"c.parquet\")
";
        let got: Vec<(String, String, String, Option<String>)> = extract_source_refs(text)
            .into_iter()
            .map(|r| {
                (
                    r.source_name.to_string(),
                    r.constructor.to_string(),
                    r.raw_uri,
                    r.option,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "A".to_string(),
                    "csv".to_string(),
                    "a.csv".to_string(),
                    Some("|".to_string())
                ),
                (
                    "B".to_string(),
                    "csv".to_string(),
                    "b.csv".to_string(),
                    None
                ),
                (
                    "C".to_string(),
                    "parquet".to_string(),
                    "c.parquet".to_string(),
                    None
                ),
            ]
        );
    }

    /// **Introspection reads the same file the run reads.**
    ///
    /// This is the whole reason `io.csv` grew a delimiter rather than a
    /// conversion step. The `DESCRIBE` runs BEFORE the compile and is what
    /// types every column the checker sees, so a `DESCRIBE` that used a
    /// different delimiter than execution would infer a schema the executor
    /// never produces — here, one column called `id|name|city` where the run
    /// yields three. That is not a slow path: the mapping's `User.name` would
    /// be refused as an unknown column, with a did-you-mean offering
    /// `id|name|city`.
    ///
    /// It asserts the COLUMNS and not the SQL, because the SQL is a rendering
    /// and the columns are the claim.
    #[test]
    fn a_pipe_delimited_source_introspects_to_the_columns_it_has() {
        let dir = tempfile::tempdir().expect("a tempdir");
        let csv = dir.path().join("people.csv");
        std::fs::write(&csv, "id|name|city\n1|Ada|Kent\n2|Bo|Arles\n").expect("write the fixture");

        let system = NativeSystem::default();
        let text = format!("User := io.csv(\"{}\", delimiter = \"|\")\n", csv.display());
        let urls = HashMap::new();
        let anchor = SourceAnchor::new(dir.path(), &urls);
        pre_introspect_and_register(&system, &text, anchor, &HashMap::new(), Reach::Anywhere);

        let descriptor = system
            .descriptors()
            .expect("NativeSystem keeps a cache")
            .get(&csv.display().to_string())
            .expect("the source was introspected");
        let names: Vec<&str> = descriptor.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["id", "name", "city"],
            "the DESCRIBE must use the delimiter the binding wrote"
        );
    }

    /// The `@conn` cases this file used to assert against its own resolver
    /// live beside the rule itself, in `fossil_locator` — there is one
    /// implementation, so there is one place to test it. What is left here is
    /// the host's own half: the projection the anchor is built from, which is
    /// the LAST thing a credential touches before the host sees only
    /// URLs.
    ///
    /// The fixture is `fossil_base::test_support::NativeSystem` and not
    /// `fossil-cli`'s `EngineSystem`, which is what these tests used when they
    /// lived there. Both are a `System` with a descriptor cache; `NativeSystem`
    /// is the one that exists so a test can have one without a host crate, and
    /// taking `fossil-cli` as a dev-dependency here would close the loop these
    /// tests are part of opening.
    #[test]
    fn the_creds_map_projects_onto_the_anchor_the_rule_takes() {
        let c = conns(&[("sales", "s3://bucket/prefix")]);
        let urls = connection_urls(&c);
        let dir = std::path::PathBuf::from("/programs/shop");
        assert_eq!(
            SourceAnchor::new(&dir, &urls).locator("@sales/2024/orders.csv"),
            "s3://bucket/prefix/2024/orders.csv"
        );
        assert_eq!(
            SourceAnchor::new(&dir, &urls).locator("data/items.csv"),
            "/programs/shop/data/items.csv"
        );
    }

    /// The cache's done-when, counted rather than timed: changing the CSV and
    /// re-running re-introspects; not changing it does not.
    ///
    /// `registrations()` moves only when a `DESCRIBE` actually ran, so the
    /// assertion is on the number of reads of the source and not on how long
    /// the second call took. The third write adds a column, which moves the
    /// size as well as the mtime — the token is both, so the test does not
    /// depend on the filesystem's clock resolution.
    #[test]
    fn a_source_is_re_introspected_when_it_changes_and_not_when_it_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv = dir.path().join("users.csv");
        std::fs::write(&csv, "id,name\n1,ada\n").expect("write csv");
        let program = "users := io.csv(\"users.csv\")\n";
        let no_creds = HashMap::new();

        let system = NativeSystem::default();
        let cache = system.descriptors().expect("the engine keeps a table");

        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &no_creds,
            Reach::Anywhere,
        );
        assert_eq!(
            cache.registrations(),
            1,
            "the first compile reads the source"
        );
        assert_eq!(cache.get("users.csv").expect("registered").columns.len(), 2);

        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &no_creds,
            Reach::Anywhere,
        );
        assert_eq!(
            cache.registrations(),
            1,
            "an untouched source must not be read a second time"
        );

        std::fs::write(&csv, "id,name,email\n1,ada,ada@example.org\n").expect("rewrite csv");
        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &no_creds,
            Reach::Anywhere,
        );
        assert_eq!(
            cache.registrations(),
            2,
            "a changed source must be read again"
        );
        assert_eq!(
            cache.get("users.csv").expect("registered").columns.len(),
            3,
            "and the new column is visible to the checker"
        );
    }

    /// The key is the URI, so two bindings over one file cost one read — the
    /// case a binding-name key charged twice for.
    #[test]
    fn two_bindings_over_one_file_introspect_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("u.csv"), "id\n1\n").expect("write csv");
        let program = "a := io.csv(\"u.csv\")\nb := io.csv(\"u.csv\")\n";

        let system = NativeSystem::default();
        let cache = system.descriptors().expect("the engine keeps a table");
        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &HashMap::new(),
            Reach::Anywhere,
        );

        assert_eq!(cache.registrations(), 1);
        assert_eq!(cache.len(), 1);
    }

    /// [`Reach::Local`] reads the file beside the program and does not reach
    /// for the URL — and, crucially, it does not reach for it AGAIN on the next
    /// call.
    ///
    /// The second half is the one with teeth. An unreachable locator has an
    /// empty freshness token and an empty token is never fresh, so under
    /// `Reach::Anywhere` every call opens a connection and attempts the read.
    /// That is right for a command and ruinous for an editor, where "every
    /// call" is every keystroke. `registrations()` cannot see it — a failed
    /// DESCRIBE registers nothing either way — so the assertion is on the local
    /// source's count staying at 1 while the remote one never appears at all.
    #[test]
    fn the_local_reach_skips_what_it_cannot_stat_and_keeps_skipping_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("near.csv"), "id,name\n1,ada\n").expect("write csv");
        let program = "near := io.csv(\"near.csv\")\n\
                       far  := io.csv(\"https://example.invalid/far.csv\")\n";

        let system = NativeSystem::default();
        let cache = system.descriptors().expect("the host keeps a table");
        for _ in 0..3 {
            pre_introspect_and_register(
                &system,
                program,
                SourceAnchor::beside(dir.path()),
                &HashMap::new(),
                Reach::Local,
            );
        }

        assert_eq!(
            cache.registrations(),
            1,
            "the CSV beside the program is read once and the URL is never read"
        );
        assert_eq!(cache.get("near.csv").expect("registered").columns.len(), 2);
        assert!(
            cache.get("https://example.invalid/far.csv").is_none(),
            "an editor must not go on the network from its message loop"
        );
    }

    /// A source this host cannot `stat` gets an empty token, and an empty token
    /// is never fresh — so a remote object is re-introspected rather than
    /// trusted. The assertion is on the token, since the DESCRIBE of an
    /// unreachable URL fails and registers nothing.
    #[test]
    fn a_locator_that_cannot_be_stat_ed_yields_no_token() {
        assert_eq!(freshness_token("https://example.org/users.csv"), "");
        assert_eq!(freshness_token("/nonexistent/users.csv"), "");
    }

    #[test]
    fn the_token_moves_when_the_file_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv = dir.path().join("u.csv");
        std::fs::write(&csv, "id\n1\n").expect("write");
        let path = csv.to_string_lossy().into_owned();
        let first = freshness_token(&path);
        assert!(!first.is_empty(), "a local file has a token");
        assert_eq!(first, freshness_token(&path), "and it is stable");

        std::fs::write(&csv, "id,name\n1,ada\n").expect("rewrite");
        assert_ne!(first, freshness_token(&path));
    }
}
