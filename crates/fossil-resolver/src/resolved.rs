//! [`ResolvedPath`] — a URL + secret-bearing cloud config the runtime
//! threads from `PathResolver::resolve` to `DuckDB`.
//!
//! Sub-path derivation ([`ResolvedPath::join`]) inherits credentials so
//! a single resolver call against `s3://bucket/prefix` can spawn many
//! per-vertex / per-edge file paths without re-resolving.

use std::collections::HashMap;

use secrecy::{ExposeSecret, SecretString};

/// A resolved external path — physical URL + per-provider auth/config the
/// runtime needs to actually open it.
///
/// The `cloud_config` map keys are provider-specific (`DuckDB` documents the
/// shape for each storage backend — e.g. `s3_access_key_id`,
/// `azure_storage_account_key`, `gcs_hmac_key_id`). Values are wrapped in
/// [`SecretString`] so they cannot accidentally land in a `Debug`-formatted
/// log line.
#[derive(Clone)]
pub struct ResolvedPath {
    url: String,
    cloud_config: HashMap<String, SecretString>,
}

impl ResolvedPath {
    /// New resolved path with no cloud credentials (the standalone /
    /// public-URL / local-file case).
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            cloud_config: HashMap::new(),
        }
    }

    /// New resolved path carrying provider-specific cloud configuration.
    /// Secrets ride in [`SecretString`] from the call site so the resolver
    /// crate never sees a raw `String` for them.
    #[must_use]
    pub fn with_config(
        url: impl Into<String>,
        cloud_config: HashMap<String, SecretString>,
    ) -> Self {
        Self {
            url: url.into(),
            cloud_config,
        }
    }

    /// Derive a sub-path that inherits the parent's credentials.
    ///
    /// `rel` is **always relative** — it is appended to the parent URL with
    /// exactly one separating `/`, regardless of whether either side has
    /// leading/trailing slashes. (Unix-style absolute semantics where
    /// `/x` overrides the base path are deliberately NOT supported — they
    /// produce surprises against `s3://` / `az://` URLs whose authority
    /// segment is part of the prefix.) The URL scheme is preserved
    /// verbatim — no encoding — because cloud URLs admit characters
    /// (path-style container names, `+` in S3 keys) that a generic
    /// `url::Url` parser would mangle.
    #[must_use]
    pub fn join(&self, rel: &str) -> Self {
        let base = self.url.trim_end_matches('/');
        let tail = rel.trim_start_matches('/');
        let url = format!("{base}/{tail}");
        Self {
            url,
            // Clone the SecretString map — each clone holds its own zero-on-drop
            // copy, so a downstream Drop doesn't invalidate the parent's secrets.
            cloud_config: self.cloud_config.clone(),
        }
    }

    /// The physical URL — `file://…`, `s3://…`, `https://…`, etc.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Provider-specific cloud configuration. Keys are DuckDB-spelt
    /// (`azure_storage_account_name`, `s3_region`, …); values are kept
    /// secret-shaped. Callers that need to apply them via `DuckDB`'s
    /// `SET key='value'` use [`for_each_setting`] which controls the
    /// expose-and-format dance in one place.
    ///
    /// [`for_each_setting`]: Self::for_each_setting
    #[must_use]
    pub const fn cloud_config(&self) -> &HashMap<String, SecretString> {
        &self.cloud_config
    }

    /// Visit each `(key, exposed_value)` pair exactly once for a
    /// configuration apply step — typically the `DuckDB` `SET` loop in the
    /// runtime. Callers do not see the raw `SecretString` map; they
    /// receive a borrowed `&str` for the value that lives only for the
    /// callback duration, after which it is no longer referenced.
    ///
    /// This is the recommended consumption path. Direct access via
    /// [`cloud_config`](Self::cloud_config) is exposed for callers that
    /// must own the map (e.g. the keasy-side helper that builds Polars
    /// `CloudOptions` during the W0d transition).
    pub fn for_each_setting<F>(&self, mut f: F)
    where
        F: FnMut(&str, &str),
    {
        for (k, v) in &self.cloud_config {
            f(k, v.expose_secret());
        }
    }
}

impl std::fmt::Debug for ResolvedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the cloud_config values — even via Debug — because the
        // map's keys alone are useful for diagnostics (you see WHICH provider
        // shape the resolver returned) without leaking the auth material.
        let redacted_keys: Vec<&String> = self.cloud_config.keys().collect();
        f.debug_struct("ResolvedPath")
            .field("url", &self.url)
            .field("cloud_config_keys", &redacted_keys)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_map(pairs: &[(&str, &str)]) -> HashMap<String, SecretString> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), SecretString::from((*v).to_string())))
            .collect()
    }

    #[test]
    fn new_carries_url_and_no_config() {
        let r = ResolvedPath::new("file:///tmp/users.csv");
        assert_eq!(r.url(), "file:///tmp/users.csv");
        assert!(r.cloud_config().is_empty());
    }

    #[test]
    fn join_appends_slash_when_missing() {
        let base = ResolvedPath::new("s3://bucket/prefix");
        assert_eq!(
            base.join("file.parquet").url(),
            "s3://bucket/prefix/file.parquet"
        );
    }

    #[test]
    fn join_does_not_double_slash() {
        // Trailing slash on base + plain rel → exactly one separator.
        let base = ResolvedPath::new("s3://bucket/prefix/");
        assert_eq!(
            base.join("file.parquet").url(),
            "s3://bucket/prefix/file.parquet"
        );
        // Leading slash on rel is stripped (NOT treated as absolute — see
        // the `join` doc comment for why the unix semantics are rejected).
        let base = ResolvedPath::new("s3://bucket/prefix");
        assert_eq!(
            base.join("/file.parquet").url(),
            "s3://bucket/prefix/file.parquet"
        );
        // Both sides loaded with slashes still collapses to one.
        let base = ResolvedPath::new("s3://bucket/prefix/");
        assert_eq!(
            base.join("/file.parquet").url(),
            "s3://bucket/prefix/file.parquet"
        );
    }

    #[test]
    fn join_inherits_cloud_config() {
        let r = ResolvedPath::with_config(
            "s3://bucket/prefix",
            secret_map(&[("s3_region", "eu-west-1")]),
        );
        let child = r.join("v.parquet");
        assert_eq!(child.url(), "s3://bucket/prefix/v.parquet");
        // The child holds the same key set (we don't assert value equality
        // because that would require expose_secret() in the assertion).
        assert!(child.cloud_config().contains_key("s3_region"));
    }

    #[test]
    fn for_each_setting_visits_all_keys_with_exposed_values() {
        let r = ResolvedPath::with_config(
            "s3://bucket",
            secret_map(&[
                ("s3_access_key_id", "AKIA..."),
                ("s3_secret_access_key", "very-secret"),
            ]),
        );
        let mut seen: Vec<(String, String)> = Vec::new();
        r.for_each_setting(|k, v| seen.push((k.to_string(), v.to_string())));
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("s3_access_key_id".to_string(), "AKIA...".to_string()),
                (
                    "s3_secret_access_key".to_string(),
                    "very-secret".to_string()
                ),
            ]
        );
    }

    #[test]
    fn debug_redacts_values() {
        let r = ResolvedPath::with_config(
            "s3://bucket",
            secret_map(&[("s3_secret_access_key", "should-not-appear")]),
        );
        let s = format!("{r:?}");
        assert!(s.contains("s3_secret_access_key"), "key visible: {s}");
        assert!(
            !s.contains("should-not-appear"),
            "value leaked in Debug: {s}",
        );
    }
}
