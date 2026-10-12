//! The editor's host for [`fossil_profile`]: the columns of every source a
//! buffer reads, described on a native `DuckDB` connection before the compile.
//!
//! The plan is the profiler's — which queries describe a source, and what their
//! answers mean — and the browser runs the same plan on its own DuckDB-WASM
//! (`/docs/design/profile`). What is this host's is the connection, and when to
//! skip the work: a source this host cannot `stat`, and one unchanged since it
//! was last described. The editor asks for [`Options::TYPES`] — one `DESCRIBE` a
//! source — because it pays on every keystroke; the counts, keys and
//! inclusions are a whole profile, and the editor does not need them.
//!
//! It fills a cache and returns nothing; a compile reads that cache through
//! `System`. The order — describe, then compile — is the caller's, and it is the
//! order the browser has always used.

use std::collections::HashSet;

use fossil_base::System;
use fossil_lineage::Input;
use fossil_profile::{Options, Rows, Source};

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
/// `s3://` location, or a path that does not exist. An empty token is never
/// fresh ([`fossil_descriptors_input::DescriptorCache::is_fresh`]), so those are
/// re-introspected on every compile. That is the honest answer for an object we
/// would have to make a network round trip to interrogate.
fn etag(resolved: &str) -> String {
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

/// One query's answer on `conn`, every cell as text: the profiler's queries
/// cast what they select to `VARCHAR`, and a `DESCRIBE`'s columns are text.
fn rows(conn: &duckdb::Connection, sql: &str) -> duckdb::Result<Rows> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let n = row.as_ref().column_count();
        out.push(
            (0..n)
                .map(|i| row.get::<_, Option<String>>(i))
                .collect::<duckdb::Result<_>>()?,
        );
    }
    Ok(out)
}

/// Register a descriptor on `system`'s cache for every source in `sources`
/// that reads through a native reader, BEFORE typecheck — keyed by
/// [`Input::key`], what the program wrote, and read from [`Input::location`].
/// One `DESCRIBE` per key: two bindings over one file are one descriptor.
///
/// A source whose cached descriptor still carries the current [`etag`] is
/// skipped — no `DESCRIBE`, no read. That is where the cost is: programs are
/// small and sources are not, so describing is the expensive half of a compile
/// and the half that rarely needs doing twice.
///
/// A source that cannot be described is logged and skipped; the compile may
/// still succeed with no forward propagation for it.
///
/// **Only a source this host can `stat` is read**: a local file that exists
/// right now. The message loop is one sequential loop over one channel, so a
/// `didOpen` blocked on `s3://` is not one slow file — it is hover and
/// completion dead in every other buffer for as long as the read takes. The
/// discriminator is [`etag`] and not a scheme test, because a location this
/// host cannot `stat` is exactly one whose freshness it cannot establish —
/// which also covers a relative path that does not exist yet, and the
/// half-typed one a keystroke produces on the way to it.
pub fn profile_and_register(system: &dyn System, sources: &[Input]) {
    let Some(cache) = system.descriptors() else {
        tracing::debug!("host keeps no descriptor cache; not describing sources");
        return;
    };
    let mut seen = HashSet::new();
    let stale: Vec<Source> = sources
        .iter()
        .filter_map(|input| {
            // The constructor chooses the reader; a materialised row (`io.rdf`)
            // takes its schema from its shape and has nothing to describe.
            let format = input.format.as_deref()?;
            fossil_profile::native_reader(format)?;
            input.binding.as_ref()?;
            if !seen.insert(input.key.as_str()) {
                return None;
            }
            // An empty token means this host could not `stat` the location. It
            // is checked BEFORE freshness: an empty token is never fresh, so
            // without this the editor would open a connection and attempt the
            // read on every keystroke.
            let token = etag(&input.location);
            if token.is_empty() {
                tracing::debug!(
                    "`{}` is not a file this host can stat; not reading it from here",
                    input.location
                );
                return None;
            }
            if cache.is_fresh(&input.key, &token) {
                return None;
            }
            Some(Source {
                key: input.key.as_str().into(),
                name: input.location.clone(),
                format: format.to_string(),
                option: input.option.clone(),
                etag: token,
            })
        })
        .collect();
    // A compile whose sources are all fresh does no `DuckDB` work at all, and
    // opening a connection is work.
    if stale.is_empty() {
        return;
    }
    let conn = match duckdb::Connection::open_in_memory() {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!("DuckDB in-memory open failed; not describing sources: {e}");
            return;
        }
    };
    let profile = fossil_profile::run(stale, Options::TYPES, |sql| {
        rows(&conn, sql).map_err(|e| e.to_string())
    });
    for failure in profile.failed {
        tracing::warn!(
            "could not describe `{}`: {}",
            failure.source,
            failure.reason
        );
    }
    for source in profile.sources {
        tracing::debug!("described `{}`", source.descriptor.key);
        cache.insert(source.descriptor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_base::test_support::NativeSystem;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    /// What `fossil_lineage::inputs` reports for `program` written at
    /// `dir/prog.fossil` — the list every host introspects.
    fn sources_of(dir: &Path, program: &str, connections: &HashMap<String, String>) -> Vec<Input> {
        let db = fossil_base::FossilDb::new(Arc::new(NativeSystem::default()));
        let file = fossil_base::SourceFile::new(
            &db,
            program.to_string(),
            dir.join("prog.fossil").to_string_lossy().into_owned(),
        );
        fossil_lineage::inputs(&db, file, connections)
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
        let sources = sources_of(dir.path(), &text, &HashMap::new());
        profile_and_register(&system, &sources);

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
        let sources = sources_of(
            dir.path(),
            "users := io.csv(\"users.csv\")\n",
            &HashMap::new(),
        );

        let system = NativeSystem::default();
        let cache = system.descriptors().expect("the engine keeps a table");

        profile_and_register(&system, &sources);
        assert_eq!(
            cache.registrations(),
            1,
            "the first compile reads the source"
        );
        assert_eq!(cache.get("users.csv").expect("registered").columns.len(), 2);

        profile_and_register(&system, &sources);
        assert_eq!(
            cache.registrations(),
            1,
            "an untouched source must not be read a second time"
        );

        std::fs::write(&csv, "id,name,email\n1,ada,ada@example.org\n").expect("rewrite csv");
        profile_and_register(&system, &sources);
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
        let sources = sources_of(
            dir.path(),
            "a := io.csv(\"u.csv\")\nb := io.csv(\"u.csv\")\n",
            &HashMap::new(),
        );

        let system = NativeSystem::default();
        let cache = system.descriptors().expect("the engine keeps a table");
        profile_and_register(&system, &sources);

        assert_eq!(cache.registrations(), 1);
        assert_eq!(cache.len(), 1);
    }

    /// A source this host cannot `stat` is skipped — and, crucially, skipped
    /// AGAIN on the next call.
    ///
    /// The second half is the one with teeth. An unreachable location has an
    /// empty freshness token and an empty token is never fresh, so reading it
    /// would open a connection and attempt the read on every call — ruinous
    /// for an editor, where "every call" is every keystroke. `registrations()` cannot see it — a failed
    /// DESCRIBE registers nothing either way — so the assertion is on the local
    /// source's count staying at 1 while the remote one never appears at all.
    #[test]
    fn what_cannot_be_stat_ed_is_skipped_and_stays_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("near.csv"), "id,name\n1,ada\n").expect("write csv");
        let sources = sources_of(
            dir.path(),
            "near := io.csv(\"near.csv\")\n\
             far  := io.csv(\"https://example.invalid/far.csv\")\n",
            &HashMap::new(),
        );

        let system = NativeSystem::default();
        let cache = system.descriptors().expect("the host keeps a table");
        for _ in 0..3 {
            profile_and_register(&system, &sources);
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

    /// The descriptor is keyed by what the program wrote and read from where
    /// the connection points: repointing `@lake` moves the read, not the key
    /// the checker looks up.
    #[test]
    fn a_conn_source_is_keyed_as_written_and_read_where_it_resolves() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("u.csv"), "id,name\n1,ada\n").expect("write csv");
        let lake = HashMap::from([("lake".to_string(), dir.path().display().to_string())]);
        let sources = sources_of(
            Path::new("/elsewhere"),
            "u := io.csv(\"@lake/u.csv\")\n",
            &lake,
        );

        let system = NativeSystem::default();
        profile_and_register(&system, &sources);

        let cache = system.descriptors().expect("the host keeps a table");
        assert_eq!(
            cache.get("@lake/u.csv").expect("registered").columns.len(),
            2
        );
    }

    /// A materialised row takes its schema from its shape: the file behind a
    /// destructured `io.rdf` is not described, even when a reader could open it.
    #[test]
    fn a_materialised_source_is_not_described() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("g.ttl"), "id\n1\n").expect("write graph");
        let sources = sources_of(
            dir.path(),
            "{ A, B } := io.rdf(\"g.ttl\", schema = io.shex(\"s.shex\"))\n",
            &HashMap::new(),
        );
        let data = sources
            .iter()
            .filter(|i| i.role == fossil_lineage::Role::Data)
            .count();
        assert_eq!(data, 2, "one data input per destructured binding");

        let system = NativeSystem::default();
        profile_and_register(&system, &sources);

        assert_eq!(system.descriptors().expect("a table").registrations(), 0);
    }

    /// A source this host cannot `stat` gets an empty token, and an empty token
    /// is never fresh — so a remote object is re-introspected rather than
    /// trusted. The assertion is on the token, since the DESCRIBE of an
    /// unreachable URL fails and registers nothing.
    #[test]
    fn a_location_that_cannot_be_stat_ed_yields_no_token() {
        assert_eq!(etag("https://example.org/users.csv"), "");
        assert_eq!(etag("/nonexistent/users.csv"), "");
    }

    #[test]
    fn the_token_moves_when_the_file_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv = dir.path().join("u.csv");
        std::fs::write(&csv, "id\n1\n").expect("write");
        let path = csv.to_string_lossy().into_owned();
        let first = etag(&path);
        assert!(!first.is_empty(), "a local file has a token");
        assert_eq!(first, etag(&path), "and it is stable");

        std::fs::write(&csv, "id,name\n1,ada\n").expect("rewrite");
        assert_ne!(first, etag(&path));
    }
}
