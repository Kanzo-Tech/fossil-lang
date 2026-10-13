//! The test host's half of [`fossil_profile`]: every source a program reads,
//! described on a native `DuckDB` connection before the compile, and written to
//! the database's source registry, the input the checker reads. The plan is the profiler's; the browser
//! runs it on DuckDB-WASM (`/docs/design/profile`). What is this host's is only
//! the connection.

use std::collections::HashSet;

use fossil_base::Db;
use fossil_hir::lineage::Input;
use fossil_profile::{Options, Rows, Source};

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

/// Describe each source that reads through a native reader and exists on disk,
/// once per key, and register what came back. A source that cannot be described
/// is skipped: the compile then has no forward propagation for it, which is what
/// the program's expected diagnostics record.
pub(crate) fn profile_and_register(db: &mut dyn Db, sources: &[Input]) {
    let mut seen = HashSet::new();
    let wanted: Vec<Source> = sources
        .iter()
        .filter_map(|input| {
            // A materialised row (`io.rdf`) takes its schema from its shape.
            let format = input.format.as_deref()?;
            fossil_profile::native_reader(format)?;
            input.binding.as_ref()?;
            if !std::path::Path::new(&input.location).is_file() || !seen.insert(input.key.as_str())
            {
                return None;
            }
            Some(Source {
                key: input.key.as_str().into(),
                name: input.location.clone(),
                format: format.to_string(),
                option: input.option.clone(),
                etag: String::new(),
            })
        })
        .collect();
    if wanted.is_empty() {
        return;
    }
    let conn = duckdb::Connection::open_in_memory().expect("an in-memory DuckDB");
    let profile = fossil_profile::run(wanted, Options::TYPES, |sql| {
        rows(&conn, sql).map_err(|e| e.to_string())
    });
    for source in profile.sources {
        fossil_base::describe_columns(db, source.descriptor);
    }
}
