//! `fossil-introspect` — what a NATIVE host does before it asks the compiler to
//! compile: read each source's columns, and hold the credentials that let it.
//!
//! # Why this is a crate and not a module of `fossil-engine`
//!
//! Because the compiler does not introspect. **The host does**, and one host
//! already proved it: `fossil-wasm` implements
//! [`fossil_base::System::descriptors`], the browser runs
//! `@fossil-lang/introspect` against its own DuckDB-WASM, and the registered
//! descriptors are all the checker ever sees. The native side did the same job
//! from *inside* `fossil-engine`, which is what made that crate open a `DuckDB`
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
//! credentials must not cross the wasm boundary. So a `fossil-engine` that
//! still parsed credentials could not be wasm-clean no matter what happened to
//! `DuckDB`. Introspection and credentials are the same concern anyway: the
//! secret exists so that the `DESCRIBE` over a cloud `@conn` source
//! authenticates.
//!
//! `fossil-engine` therefore takes a `HashMap<String, String>` of connection
//! URLs and never sees a secret.
//!
//! # What this crate is NOT
//!
//! It is not a second compiler entry point. It fills a cache and returns
//! nothing; `fossil_engine::check` / `run` read that cache through `System`. The
//! order — introspect, then compile — is the caller's, and it is the order the
//! browser has always used.

use std::collections::HashMap;

use fossil_base::{SourceAnchor, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::Primitive;
use smol_str::SmolStr;

pub mod creds;

pub use creds::{ConnectionCreds, RunCreds, SecretSpec};

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
        "TIMESTAMP" | "DATETIME" => Primitive::DateTime,
        "TIME" => Primitive::Time,
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

/// Scrape source-binding RHS source URLs from a `.fossil` file's text. It is a
/// regex placeholder for an AST walk, and it is wrong on any binding the regex
/// cannot see.
///
/// `@fossil-lang/introspect` scrapes the same bindings for the browser. **The
/// alternation is no longer written here**: it is built from the catalogue, and
/// the TypeScript builds its own from `catalogue.generated.ts`, which
/// `cargo xtask catalogue` writes from the same file. A constructor added to
/// `catalogue.bnf` reaches both scrapers at once.
///
/// What is still written twice is the pattern AROUND the alternation, in two
/// regex dialects, and `packages/introspect/tests/rust-parity.test.ts` reads
/// this file for it. It is a `pnpm` test, so `cargo test` will not tell you.
fn extract_source_refs(text: &str) -> Vec<(SmolStr, SmolStr, String)> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        let alternation = native_rows().map(|p| p.name).collect::<Vec<_>>().join("|");
        regex::Regex::new(&format!(
            r#"(\w[\w\d_]*)\s*:=\s*io\.({alternation})\(\s*['"]([^'"]+)['"]"#
        ))
        .expect("the catalogue's constructor names are regex-safe")
    });
    re.captures_iter(text)
        .map(|c| {
            (
                SmolStr::from(c.get(1).unwrap().as_str()),
                SmolStr::from(c.get(2).unwrap().as_str()),
                c.get(3).unwrap().as_str().to_string(),
            )
        })
        .collect()
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

/// Pre-introspect every source the program names and register an
/// [`InferredDescriptor`] on the host's descriptor cache BEFORE typecheck,
/// keyed by the URI the program writes rather than by the resolved locator —
/// the written URI is the only string the host and the checker both see.
///
/// Introspect the program at `path` into `system`'s descriptor cache — read the
/// file, anchor it, and describe every source it binds.
///
/// **This is the unit of work a host actually has**, and the reason it is here
/// rather than repeated at each call site: `pre_introspect_and_register` takes
/// text that has already been read and an anchor that has already been built,
/// and every one of the eleven callers wanted the same three lines in front of
/// it. The `System` is the caller's because only the caller knows which host it
/// is — `fossil_engine::host_system(path)` natively, and the browser does not
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
    let program_dir = fossil_base::program_dir(&path.to_string_lossy());
    let anchor = SourceAnchor::new(&program_dir, connections);
    pre_introspect_and_register(system, &text, anchor, &creds.connections);
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
#[allow(clippy::implicit_hasher)] // as `introspect_program`.
pub fn pre_introspect_and_register(
    system: &dyn System,
    source_text: &str,
    anchor: SourceAnchor<'_>,
    connections: &HashMap<String, ConnectionCreds>,
) {
    let Some(cache) = system.descriptors() else {
        tracing::debug!("host keeps no descriptor cache; skipping pre-introspection");
        return;
    };

    // Opened on the first miss, not on entry. A compile whose sources are all
    // fresh must do no DuckDB work at all, and opening a connection is work.
    let mut conn: Option<duckdb::Connection> = None;

    for (source_name, constructor, raw_uri) in extract_source_refs(source_text) {
        let token = freshness_token(&anchor.locator(&raw_uri));
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
        let Some(reader) = native_rows()
            .find(|p| p.name == constructor.as_str())
            .and_then(|p| match p.reads_rows {
                Some(fossil_base::RowReader::Native(r)) => Some(r.table_function()),
                _ => None,
            })
        else {
            tracing::warn!(
                "source `{source_name}` names `io.{constructor}`, which is not a \
                 natively-readable catalogue row; skipping pre-introspection"
            );
            continue;
        };
        let sql = format!("DESCRIBE SELECT * FROM {reader}('{escaped_path}')");
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
/// [`fossil_base::SourceAnchor`] expands a `@conn` alias through, and what the
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
/// This ran through `fossil_runtime::install_secret`, and that indirection is
/// gone. The rendering — which is the part with a decision in it, and the part
/// with tests — is [`fossil_resolver::ResolvedPath::create_secret_sql`], in `fossil-resolver`,
/// and it has not moved. What wrapped it was `conn.execute_batch(sql)` under an
/// error enum that both of its two callers immediately flattened to a string.
/// A crate does not need a dependency to run one statement on a connection it
/// already holds, and that dependency was the last thing making `fossil-runtime`
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

    /// The four `@conn` cases this file used to assert against its own resolver
    /// live beside the rule itself, in `fossil_base::locator` — there is one
    /// implementation, so there is one place to test it. What is left here is
    /// the host's own half: the projection the anchor is built from, which is
    /// the LAST thing a credential touches before `fossil-engine` sees only
    /// URLs.
    ///
    /// The fixture is `fossil_base::test_support::NativeSystem` and not
    /// `fossil-engine`'s `EngineSystem`, which is what these tests used when they
    /// lived there. Both are a `System` with a descriptor cache; `NativeSystem`
    /// is the one that exists so a test can have one without a host crate, and
    /// taking `fossil-engine` as a dev-dependency here would close the loop these
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
        );

        assert_eq!(cache.registrations(), 1);
        assert_eq!(cache.len(), 1);
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
