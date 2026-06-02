//! Credential intake for `fossil run --creds-stdin`.
//!
//! Cloud credentials must never ride argv or the environment — both are
//! world-readable on a shared host via `ps` / `/proc/<pid>/{cmdline,environ}`.
//! A multi-tenant host (keasy) instead pipes a single JSON document on **stdin**
//! carrying the `DuckDB` cloud-config for the destination and for each `@conn`
//! source. The CLI applies it through the resolver's existing
//! `SET <key>='<value>'` dance ([`fossil_resolver::ResolvedPath::for_each_setting`])
//! — one mechanism for reading sources and writing the destination.
//!
//! Wire shape (the dest section; a `connections` section for `@conn` source
//! resolution joins it in slice 3 — unknown fields are ignored until then):
//!
//! ```json
//! { "dest": { "config": { "s3_region": "eu-west-1", "s3_access_key_id": "…" } } }
//! ```
//!
//! Keys are `DuckDB`-spelt (`s3_access_key_id`, `azure_storage_account_key`, …) —
//! the host owns the provider→`DuckDB`-key projection; fossil applies the keys
//! verbatim. Values are [`SecretString`] so a stray `Debug` never leaks them.

// `main.rs` declares this private `mod creds;`. Items `main` reads must be
// `pub(crate)` to satisfy `unreachable_pub`; in a private module that trips the
// inverse `redundant_pub_crate` nursery lint, which we silence here (same combo
// as `fossil-syntax::parser::expr`). The visibility IS correct.
#![allow(clippy::redundant_pub_crate)]

use std::collections::HashMap;
use std::io::Read;

use secrecy::SecretString;
use serde::Deserialize;

/// The `--creds-stdin` payload. Defaults to empty so an absent `dest` section
/// is the no-cloud-config case (local / public URLs).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct RunCreds {
    /// Cloud config for the `--dest` URL (addressed out-of-band on the CLI).
    #[serde(default)]
    pub(crate) dest: EndpointCreds,
}

/// Cloud config for an endpoint whose URL is supplied separately (the dest).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct EndpointCreds {
    /// `DuckDB`-spelt cloud config keys → secret values.
    #[serde(default)]
    pub(crate) config: HashMap<String, SecretString>,
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
        assert!(creds.dest.config.is_empty());
    }

    #[test]
    fn unknown_sections_are_ignored() {
        // A `connections` section (slice 3) parses today as an ignored unknown
        // field — forward-compatible with the host sending the full payload.
        let creds = RunCreds::from_json(
            r#"{ "dest": { "config": {} }, "connections": { "x": { "url": "s3://b" } } }"#,
        )
        .expect("unknown section ignored");
        assert!(creds.dest.config.is_empty());
    }

    #[test]
    fn dest_config_round_trips_as_secret() {
        let json = r#"{ "dest": { "config": { "s3_region": "eu-west-1",
                                               "s3_secret_access_key": "shhh" } } }"#;
        let creds = RunCreds::from_json(json).expect("parses");
        assert_eq!(
            creds.dest.config["s3_region"].expose_secret(),
            "eu-west-1"
        );
        assert_eq!(
            creds.dest.config["s3_secret_access_key"].expose_secret(),
            "shhh"
        );
    }

    #[test]
    fn secrets_never_appear_in_debug() {
        let creds =
            RunCreds::from_json(r#"{ "dest": { "config": { "k": "leaky-value" } } }"#).unwrap();
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
