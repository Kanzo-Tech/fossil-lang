//! [`ResolvedPath`] — a URL plus the optional [`CloudSecret`] the runtime
//! installs (via `DuckDB` `CREATE SECRET`) so a `read_*`/`COPY` under that URL
//! authenticates.
//!
//! Sub-path derivation ([`ResolvedPath::join`]) inherits the secret so a single
//! resolver call against `s3://bucket/prefix` can spawn many per-vertex /
//! per-edge file paths; one scoped secret (installed once for the prefix URL)
//! covers them all by longest-prefix match.

use std::collections::HashMap;
use std::fmt::Write as _;

use secrecy::{ExposeSecret, SecretString};

/// A `DuckDB` cloud secret: its provider `TYPE` plus the typed parameters `DuckDB`
/// `CREATE SECRET` takes (`KEY_ID`, `SECRET`, `REGION`, `ENDPOINT`, `URL_STYLE`,
/// `USE_SSL`, `CONNECTION_STRING`, …). The host owns the provider→parameter
/// projection; this crate stays provider-agnostic and just renders the
/// statement. Secret-bearing values ride [`SecretString`] so they cannot leak
/// through `Debug`.
///
/// Replaces the earlier global `SET <key>='<value>'` dance: `CREATE SECRET` is
/// typed (s3/azure/gcs all clean), and **scoped** — installing one secret per
/// connection (scope = its URL prefix) lets a single job read/write across
/// distinct cloud accounts with no last-writer-wins collision.
#[derive(Clone)]
pub struct CloudSecret {
    secret_type: String,
    params: HashMap<String, SecretString>,
}

impl CloudSecret {
    /// Construct a secret of provider `secret_type` (`"s3"`, `"azure"`, `"gcs"`)
    /// with `DuckDB` `CREATE SECRET` parameters (`"KEY_ID"` → value, …).
    #[must_use]
    pub fn new(secret_type: impl Into<String>, params: HashMap<String, SecretString>) -> Self {
        Self {
            secret_type: secret_type.into(),
            params,
        }
    }
}

impl std::fmt::Debug for CloudSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Parameter VALUES are secret — print the provider type + parameter
        // KEYS only (useful for diagnostics, leaks no auth material).
        let mut keys: Vec<&String> = self.params.keys().collect();
        keys.sort();
        f.debug_struct("CloudSecret")
            .field("secret_type", &self.secret_type)
            .field("param_keys", &keys)
            .finish()
    }
}

/// A resolved external path — physical URL + the cloud secret (if any) the
/// runtime needs to open it.
#[derive(Clone)]
pub struct ResolvedPath {
    url: String,
    secret: Option<CloudSecret>,
}

/// `TYPE`/parameter identifiers are interpolated UNQUOTED into the `CREATE
/// SECRET` statement, so guard them — they cross the host boundary (stdin) and
/// must be plain identifiers, never SQL. Values are single-quoted + escaped and
/// are safe regardless.
fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

impl ResolvedPath {
    /// New resolved path with no cloud secret (the standalone / public-URL /
    /// local-file case).
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            secret: None,
        }
    }

    /// New resolved path carrying a [`CloudSecret`]. The secret is scoped to
    /// this path's URL when installed (see [`create_secret_sql`]).
    ///
    /// [`create_secret_sql`]: Self::create_secret_sql
    #[must_use]
    pub fn with_secret(url: impl Into<String>, secret: CloudSecret) -> Self {
        Self {
            url: url.into(),
            secret: Some(secret),
        }
    }

    /// Derive a sub-path that inherits the parent's secret. `rel` is **always
    /// relative** — appended with exactly one separating `/` regardless of
    /// surrounding slashes. (Unix-absolute semantics where `/x` overrides are
    /// deliberately NOT supported — they surprise against `s3://`/`az://` URLs
    /// whose authority is part of the prefix.) The scheme is preserved verbatim.
    #[must_use]
    pub fn join(&self, rel: &str) -> Self {
        let base = self.url.trim_end_matches('/');
        let tail = rel.trim_start_matches('/');
        Self {
            url: format!("{base}/{tail}"),
            secret: self.secret.clone(),
        }
    }

    /// The physical URL — `file://…`, `s3://…`, `https://…`, etc.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether a cloud secret is attached (i.e. [`create_secret_sql`] will
    /// yield a statement).
    ///
    /// [`create_secret_sql`]: Self::create_secret_sql
    #[must_use]
    pub const fn has_secret(&self) -> bool {
        self.secret.is_some()
    }

    /// Render the `CREATE OR REPLACE SECRET <name> (...)` statement `DuckDB`
    /// installs before any `read_*`/`COPY` under this path's URL — `None` when
    /// no secret is attached (local / public). The secret is **scoped** to this
    /// path's URL, so `DuckDB` applies it by longest-prefix match and distinct
    /// per-connection secrets never collide.
    ///
    /// `CREATE OR REPLACE` so re-installing under the same `name` (e.g. a retry)
    /// is idempotent. Parameter values are single-quoted with `''` escaping;
    /// the `TYPE` and parameter identifiers are validated as plain identifiers
    /// (host-supplied over stdin) and a non-conforming one drops that parameter
    /// rather than emit injectable SQL.
    #[must_use]
    pub fn create_secret_sql(&self, name: &str) -> Option<String> {
        let secret = self.secret.as_ref()?;
        if !is_ident(&secret.secret_type) || !is_ident(name) {
            return None;
        }
        let mut sql = format!("CREATE OR REPLACE SECRET {name} (TYPE {}", secret.secret_type);
        // Sorted for a deterministic statement (tests, logs).
        let mut keys: Vec<&String> = secret.params.keys().collect();
        keys.sort();
        for k in keys {
            if !is_ident(k) {
                continue;
            }
            let v = secret.params[k].expose_secret().replace('\'', "''");
            write!(sql, ", {k} '{v}'").expect("writing to a String never fails");
        }
        let scope = self.url.replace('\'', "''");
        write!(sql, ", SCOPE '{scope}')").expect("writing to a String never fails");
        Some(sql)
    }
}

impl std::fmt::Debug for ResolvedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print secret values. The provider type + parameter KEYS are
        // useful for diagnostics (which secret shape the resolver returned)
        // without leaking the auth material.
        let secret = self.secret.as_ref().map(|s| {
            let mut keys: Vec<&String> = s.params.keys().collect();
            keys.sort();
            format!("{} {keys:?}", s.secret_type)
        });
        f.debug_struct("ResolvedPath")
            .field("url", &self.url)
            .field("secret", &secret)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_secret() -> CloudSecret {
        let mut params = HashMap::new();
        params.insert("KEY_ID".to_string(), SecretString::from("AKIA"));
        params.insert("SECRET".to_string(), SecretString::from("shh"));
        params.insert("REGION".to_string(), SecretString::from("eu-west-1"));
        CloudSecret::new("s3", params)
    }

    #[test]
    fn new_has_no_secret() {
        let r = ResolvedPath::new("file:///tmp/users.csv");
        assert_eq!(r.url(), "file:///tmp/users.csv");
        assert!(!r.has_secret());
        assert!(r.create_secret_sql("x").is_none());
    }

    #[test]
    fn create_secret_sql_is_typed_scoped_and_deterministic() {
        let r = ResolvedPath::with_secret("s3://bucket/prefix", s3_secret());
        let sql = r.create_secret_sql("__fossil_dest").expect("secret sql");
        assert_eq!(
            sql,
            "CREATE OR REPLACE SECRET __fossil_dest (TYPE s3, KEY_ID 'AKIA', \
             REGION 'eu-west-1', SECRET 'shh', SCOPE 's3://bucket/prefix')"
        );
    }

    #[test]
    fn join_inherits_secret_and_scope_stays_on_the_parent_prefix() {
        let r = ResolvedPath::with_secret("s3://bucket/prefix", s3_secret());
        let child = r.join("vertex/Person.parquet");
        assert_eq!(child.url(), "s3://bucket/prefix/vertex/Person.parquet");
        assert!(child.has_secret());
    }

    #[test]
    fn join_collapses_slashes() {
        let base = ResolvedPath::with_secret("s3://bucket/prefix/", s3_secret());
        assert_eq!(
            base.join("/v.parquet").url(),
            "s3://bucket/prefix/v.parquet"
        );
    }

    #[test]
    fn escapes_single_quotes_in_values() {
        let mut params = HashMap::new();
        params.insert("SECRET".to_string(), SecretString::from("a'b"));
        let r = ResolvedPath::with_secret("s3://b", CloudSecret::new("s3", params));
        let sql = r.create_secret_sql("s").expect("sql");
        assert!(sql.contains("SECRET 'a''b'"), "value not escaped: {sql}");
    }

    #[test]
    fn rejects_non_identifier_type_or_name() {
        let r = ResolvedPath::with_secret("s3://b", CloudSecret::new("s3; DROP", HashMap::new()));
        assert!(r.create_secret_sql("ok").is_none(), "injected TYPE not rejected");
        let r2 = ResolvedPath::with_secret("s3://b", s3_secret());
        assert!(r2.create_secret_sql("bad name").is_none(), "injected name not rejected");
    }

    #[test]
    fn debug_redacts_values() {
        let s = format!("{:?}", ResolvedPath::with_secret("s3://b", s3_secret()));
        assert!(s.contains("KEY_ID"), "key visible: {s}");
        assert!(!s.contains("AKIA"), "value leaked: {s}");
        assert!(!s.contains("shh"), "value leaked: {s}");
    }
}
