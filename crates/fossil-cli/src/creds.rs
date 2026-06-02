//! Credential intake for `fossil run --creds-stdin`.
//!
//! Cloud credentials must never ride argv or the environment — both are
//! world-readable on a shared host via `ps` / `/proc/<pid>/{cmdline,environ}`.
//! A multi-tenant host (keasy) instead pipes a single JSON document on **stdin**
//! carrying a provider-typed cloud secret for the destination and for each
//! `@conn` source. The CLI installs each via a scoped `DuckDB` `CREATE SECRET`
//! (scope = the dest / connection URL) — one mechanism for reading sources and
//! writing the destination, with no global last-writer-wins collision across
//! distinct cloud accounts.
//!
//! Wire shape:
//!
//! ```json
//! { "dest": { "secret": { "type": "s3",
//!                         "params": { "KEY_ID": "…", "SECRET": "…", "REGION": "…" } } },
//!   "connections": { "sales": { "url": "s3://bucket/prefix",
//!                               "secret": { "type": "s3", "params": { … } } } } }
//! ```
//!
//! `type` is the `DuckDB` secret provider (`s3`/`azure`/`gcs`); `params` keys are
//! `CREATE SECRET` parameter names (`KEY_ID`, `SECRET`, `REGION`, `ENDPOINT`,
//! `URL_STYLE`, `USE_SSL`, `CONNECTION_STRING`, …). The host owns the
//! provider→parameter projection; fossil renders the statement. Values are
//! [`SecretString`] so a stray `Debug` never leaks them.

// `main.rs` declares this private `mod creds;`. Items `main` reads must be
// `pub(crate)` to satisfy `unreachable_pub`; in a private module that trips the
// inverse `redundant_pub_crate` nursery lint, which we silence here (same combo
// as `fossil-syntax::parser::expr`). The visibility IS correct.
#![allow(clippy::redundant_pub_crate)]

use std::collections::HashMap;
use std::io::Read;

use fossil_resolver::CloudSecret;
use secrecy::SecretString;
use serde::Deserialize;

/// The `--creds-stdin` payload. Defaults to empty so an absent `dest`/
/// `connections` section is the no-cloud-secret case (local / public URLs).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct RunCreds {
    /// Cloud secret for the `--dest` URL (addressed out-of-band on the CLI).
    #[serde(default)]
    pub(crate) dest: EndpointCreds,
    /// Per-`@conn-name` source resolution: base URL + read secret. A `.fossil`
    /// source `io.csv("@sales/x.csv")` resolves against `connections` —
    /// `<url>/x.csv` for the read, `secret` installed scoped to `<url>`.
    #[serde(default)]
    pub(crate) connections: HashMap<String, ConnectionCreds>,
}

/// The `fossil catalog` stdin payload: the DCAT-AP catalog data + the dest
/// cloud secret (the catalog graph's destination is addressed on the CLI).
#[derive(Debug, Deserialize)]
pub(crate) struct CatalogRequest {
    /// Governance values + dataset structure the DCAT-AP graph is built from.
    pub(crate) catalog: fossil_run_status::CatalogInput,
    /// Cloud secret for the `--dest` URL; `None` ⇒ local / public dest.
    #[serde(default)]
    pub(crate) dest: EndpointCreds,
}

impl CatalogRequest {
    /// Read and parse the JSON payload from `stdin`.
    ///
    /// # Errors
    ///
    /// Returns the stringified `io`/`serde_json` error if stdin is unreadable
    /// or the payload is not a [`CatalogRequest`].
    pub(crate) fn from_stdin() -> Result<Self, String> {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("reading catalog stdin: {e}"))?;
        serde_json::from_str(&buf).map_err(|e| format!("parsing catalog JSON: {e}"))
    }
}

/// The cloud secret for an endpoint whose URL is supplied separately (the dest).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct EndpointCreds {
    /// Provider-typed secret; `None` ⇒ local / public dest.
    #[serde(default)]
    pub(crate) secret: Option<SecretSpec>,
}

/// A resolvable `@conn-name` source: its base URL plus the read secret.
#[derive(Debug, Deserialize)]
pub(crate) struct ConnectionCreds {
    /// Base URL the connection name resolves to (e.g. `s3://bucket/prefix`).
    pub(crate) url: String,
    /// Provider-typed secret; `None` ⇒ public-URL source.
    #[serde(default)]
    pub(crate) secret: Option<SecretSpec>,
}

/// A `DuckDB` `CREATE SECRET` spec: provider type + parameters.
#[derive(Debug, Deserialize)]
pub(crate) struct SecretSpec {
    /// `DuckDB` secret provider — `"s3"`, `"azure"`, `"gcs"`.
    #[serde(rename = "type")]
    pub(crate) secret_type: String,
    /// `CREATE SECRET` parameter names (`KEY_ID`, `SECRET`, `REGION`, …) → values.
    #[serde(default)]
    pub(crate) params: HashMap<String, SecretString>,
}

impl SecretSpec {
    /// Convert to a [`CloudSecret`] the resolver renders into `CREATE SECRET`.
    pub(crate) fn to_cloud_secret(&self) -> CloudSecret {
        CloudSecret::new(self.secret_type.clone(), self.params.clone())
    }
}

impl RunCreds {
    /// Read and parse the JSON payload from `stdin`. An empty stdin is a valid
    /// "no credentials" payload (returns [`RunCreds::default`]) so a caller can
    /// pass `--creds-stdin` unconditionally without special-casing local runs.
    ///
    /// # Errors
    ///
    /// Returns the stringified `io`/`serde_json` error if stdin is unreadable
    /// or the payload is not the documented shape.
    pub(crate) fn from_stdin() -> Result<Self, String> {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("reading --creds-stdin: {e}"))?;
        Self::from_json(&buf)
    }

    /// Parse the payload from a JSON string (split out for unit tests). Blank
    /// input ⇒ empty creds.
    ///
    /// # Errors
    ///
    /// Returns the stringified `serde_json` error on a malformed payload.
    pub(crate) fn from_json(s: &str) -> Result<Self, String> {
        if s.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(s).map_err(|e| format!("parsing --creds-stdin JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn blank_payload_is_empty_creds() {
        let creds = RunCreds::from_json("   \n").expect("blank parses");
        assert!(creds.dest.secret.is_none());
        assert!(creds.connections.is_empty());
    }

    #[test]
    fn connections_carry_url_and_typed_secret() {
        let creds = RunCreds::from_json(
            r#"{ "connections": { "sales": { "url": "s3://bucket/prefix",
                                             "secret": { "type": "s3",
                                                         "params": { "KEY_ID": "AKIA" } } } } }"#,
        )
        .expect("connections parse");
        let sales = &creds.connections["sales"];
        assert_eq!(sales.url, "s3://bucket/prefix");
        let secret = sales.secret.as_ref().expect("secret");
        assert_eq!(secret.secret_type, "s3");
        assert_eq!(secret.params["KEY_ID"].expose_secret(), "AKIA");
    }

    #[test]
    fn dest_secret_round_trips() {
        let json = r#"{ "dest": { "secret": { "type": "s3",
                                              "params": { "REGION": "eu-west-1",
                                                          "SECRET": "shhh" } } } }"#;
        let creds = RunCreds::from_json(json).expect("parses");
        let secret = creds.dest.secret.as_ref().expect("secret");
        assert_eq!(secret.secret_type, "s3");
        assert_eq!(secret.params["REGION"].expose_secret(), "eu-west-1");
        // The spec converts to a resolver CloudSecret that renders CREATE SECRET.
        let sql = fossil_resolver::ResolvedPath::with_secret("s3://b", secret.to_cloud_secret())
            .create_secret_sql("s")
            .expect("sql");
        assert!(sql.contains("TYPE s3"), "type missing: {sql}");
        assert!(sql.contains("REGION 'eu-west-1'"), "param missing: {sql}");
    }

    #[test]
    fn secrets_never_appear_in_debug() {
        let creds = RunCreds::from_json(
            r#"{ "dest": { "secret": { "type": "s3", "params": { "SECRET": "leaky-value" } } } }"#,
        )
        .unwrap();
        assert!(
            !format!("{creds:?}").contains("leaky-value"),
            "secret leaked through Debug"
        );
    }

    #[test]
    fn malformed_payload_errors() {
        assert!(RunCreds::from_json("{ not json").is_err());
    }
}
