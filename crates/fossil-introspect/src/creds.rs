//! Credential intake for `fossil run --creds-stdin`.
//!
//! Cloud credentials must never ride argv or the environment — both are
//! world-readable on a shared host via `ps` / `/proc/<pid>/{cmdline,environ}`.
//! A multi-tenant host instead pipes a single JSON document on **stdin**
//! carrying a provider-typed cloud secret for each `@conn` source. The engine
//! installs each via a scoped `DuckDB` `CREATE SECRET` (scope = the connection
//! URL), so distinct cloud accounts do not collide last-writer-wins.
//!
//! Wire shape:
//!
//! ```json
//! { "connections": { "sales": { "url": "s3://bucket/prefix",
//!                               "secret": { "type": "s3", "params": { … } } } } }
//! ```
//!
//! `type` is the `DuckDB` secret provider (`s3`/`azure`/`gcs`); `params` keys are
//! `CREATE SECRET` parameter names (`KEY_ID`, `SECRET`, `REGION`, `ENDPOINT`,
//! `URL_STYLE`, `USE_SSL`, `CONNECTION_STRING`, …). The host owns the
//! provider→parameter projection; fossil renders the statement. Values are
//! [`SecretString`] so a stray `Debug` never leaks them.
//!
//! # There is no `dest` section, and there was
//!
//! The payload carried `{"dest": {"secret": …}}` and nothing ever read it: the
//! `run` path resolves its destination through `local_dest_dir`, which refuses
//! every URL carrying a scheme before any credential is consulted. A field a
//! host can fill in and be ignored is worse than an absent one — it reads as
//! support for a cloud destination that does not exist.
//! `fossil-cli/tests/cloud_dest.rs` is the refusal, asserted rather than
//! described, so this paragraph cannot outlive it.

use std::collections::HashMap;
use std::io::Read;

use fossil_resolver::CloudSecret;
use secrecy::SecretString;
use serde::Deserialize;

/// The `--creds-stdin` payload. Defaults to empty so an absent `connections`
/// section is the no-cloud-secret case (local / public URLs).
#[derive(Debug, Default, Deserialize)]
pub struct RunCreds {
    /// Per-`@conn-name` source resolution: base URL + read secret. A `.fossil`
    /// source `io.csv("@sales/x.csv")` resolves against `connections` —
    /// `<url>/x.csv` for the read, `secret` installed scoped to `<url>`.
    #[serde(default)]
    pub connections: HashMap<String, ConnectionCreds>,
}

/// A resolvable `@conn-name` source: its base URL plus the read secret.
#[derive(Debug, Deserialize)]
pub struct ConnectionCreds {
    /// Base URL the connection name resolves to (e.g. `s3://bucket/prefix`).
    pub url: String,
    /// Provider-typed secret; `None` ⇒ public-URL source.
    #[serde(default)]
    pub secret: Option<SecretSpec>,
}

/// A `DuckDB` `CREATE SECRET` spec: provider type + parameters.
#[derive(Debug, Deserialize)]
pub struct SecretSpec {
    /// `DuckDB` secret provider — `"s3"`, `"azure"`, `"gcs"`.
    #[serde(rename = "type")]
    pub secret_type: String,
    /// `CREATE SECRET` parameter names (`KEY_ID`, `SECRET`, `REGION`, …) → values.
    #[serde(default)]
    pub params: HashMap<String, SecretString>,
}

impl SecretSpec {
    /// Convert to a [`CloudSecret`] the resolver renders into `CREATE SECRET`.
    pub fn to_cloud_secret(&self) -> CloudSecret {
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
    pub fn from_stdin() -> Result<Self, String> {
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
    pub fn from_json(s: &str) -> Result<Self, String> {
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

    /// A parsed secret renders the `CREATE SECRET` the resolver installs.
    ///
    /// A connection's, not a `dest`'s — nothing installs one of those. This is
    /// the secret the run actually reaches, via `apply_source_creds`.
    #[test]
    fn a_connection_secret_round_trips_into_create_secret() {
        let json = r#"{ "connections": { "sales": { "url": "s3://b",
                                                    "secret": { "type": "s3",
                                                                "params": { "REGION": "eu-west-1",
                                                                            "SECRET": "shhh" } } } } }"#;
        let creds = RunCreds::from_json(json).expect("parses");
        let secret = creds.connections["sales"].secret.as_ref().expect("secret");
        assert_eq!(secret.secret_type, "s3");
        assert_eq!(secret.params["REGION"].expose_secret(), "eu-west-1");
        let sql = fossil_resolver::ResolvedPath::with_secret("s3://b", secret.to_cloud_secret())
            .create_secret_sql("s")
            .expect("sql");
        assert!(sql.contains("TYPE s3"), "type missing: {sql}");
        assert!(sql.contains("REGION 'eu-west-1'"), "param missing: {sql}");
    }

    #[test]
    fn secrets_never_appear_in_debug() {
        let creds = RunCreds::from_json(
            r#"{ "connections": { "sales": { "url": "s3://b",
                                             "secret": { "type": "s3",
                                                         "params": { "SECRET": "leaky-value" } } } } }"#,
        )
        .unwrap();
        assert!(
            !format!("{creds:?}").contains("leaky-value"),
            "secret leaked through Debug"
        );
    }

    /// An unknown key is not an error, and that is the reason a `dest` section
    /// could sit in the payload for as long as it did without anything
    /// noticing. Asserted rather than assumed: a host still sending one gets a
    /// run, not a parse failure, and the refusal it deserves comes from
    /// `fossil-cli/tests/cloud_dest.rs` instead.
    #[test]
    fn a_section_nothing_reads_is_silently_ignored() {
        let creds = RunCreds::from_json(r#"{ "dest": { "secret": { "type": "s3" } } }"#)
            .expect("an unknown section parses");
        assert!(creds.connections.is_empty());
    }

    #[test]
    fn malformed_payload_errors() {
        assert!(RunCreds::from_json("{ not json").is_err());
    }
}
