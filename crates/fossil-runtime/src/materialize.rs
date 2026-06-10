//! Cloud-secret installation for a `DuckDB` connection — scoping a destination
//! or source's `CREATE SECRET` so a later `read_*` over a cloud URL
//! authenticates. The native GraphAr *writer* (the DuckDB COPY executor) was
//! retired when both the run and catalog paths moved to the `fossil-df`
//! (DataFusion/Arrow) materializer; this module now carries only the secret
//! seam, still shared by the CLI (`@conn` source creds) and `fossil-mcp`.

use duckdb::Connection;
use fossil_resolver::ResolvedPath;

/// Errors installing a destination's scoped cloud secret on a `DuckDB`
/// connection.
#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    /// Installing the scoped `CREATE SECRET` failed. Usually means a secret
    /// parameter the resolver supplied is rejected by `DuckDB` (typo, missing
    /// extension).
    #[error("installing cloud secret `{name}` failed: {source}")]
    Secret { name: String, source: duckdb::Error },
}


/// Install a [`ResolvedPath`]'s scoped `CREATE SECRET` on a `DuckDB` connection
/// under the given `name`, BEFORE any `read_*`/`COPY` that dereferences a cloud
/// URL under it. A no-op when the path carries no secret (local / public URLs).
/// Shared by [`materialize`] (dest writes) and host callers that read cloud
/// sources (e.g. the CLI's `@conn/path` source resolution), so secret rendering
/// lives in exactly one place ([`ResolvedPath::create_secret_sql`]).
///
/// The secret is scoped to the path's URL, so installing one per connection
/// lets a single job span distinct cloud accounts with no collision — `DuckDB`
/// applies each by longest-prefix scope match.
///
/// # Errors
///
/// Returns [`MaterializeError::Secret`] if `DuckDB` rejects the statement.
pub fn install_secret(
    conn: &Connection,
    path: &ResolvedPath,
    name: &str,
) -> Result<(), MaterializeError> {
    if let Some(sql) = path.create_secret_sql(name) {
        conn.execute_batch(&sql)
            .map_err(|source| MaterializeError::Secret {
                name: name.to_string(),
                source,
            })?;
    }
    Ok(())
}

