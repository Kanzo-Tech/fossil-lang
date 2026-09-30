//! Connection credentials for a native host's introspection.
//!
//! A `@conn` source carries a provider-typed cloud secret. The engine installs
//! each via a scoped `DuckDB` `CREATE SECRET` (scope = the connection URL), so
//! distinct cloud accounts do not collide last-writer-wins.
//!
//! `secret_type` is the `DuckDB` secret provider (`s3`/`azure`/`gcs`); `params`
//! keys are `CREATE SECRET` parameter names (`KEY_ID`, `SECRET`, `REGION`,
//! `ENDPOINT`, `URL_STYLE`, `USE_SSL`, `CONNECTION_STRING`, …). The host owns
//! the provider→parameter projection; fossil renders the statement. Values are
//! [`SecretString`] so a stray `Debug` never leaks them.
//!
//! # There is no JSON intake, and there was
//!
//! These were read off stdin as JSON (`--creds-stdin`) by the native CLI. The
//! CLI went on 2026-09-30 and its payload with it: a host builds the map.

use std::collections::HashMap;

use fossil_storage::CloudSecret;
use secrecy::SecretString;

/// A resolvable `@conn-name` source: its base URL plus the read secret.
#[derive(Debug)]
pub struct ConnectionCreds {
    /// Base URL the connection name resolves to (e.g. `s3://bucket/prefix`).
    pub url: String,
    /// Provider-typed secret; `None` ⇒ public-URL source.
    pub secret: Option<SecretSpec>,
}

/// A `DuckDB` `CREATE SECRET` spec: provider type + parameters.
#[derive(Debug)]
pub struct SecretSpec {
    /// `DuckDB` secret provider — `"s3"`, `"azure"`, `"gcs"`.
    pub secret_type: String,
    /// `CREATE SECRET` parameter names (`KEY_ID`, `SECRET`, `REGION`, …) → values.
    pub params: HashMap<String, SecretString>,
}

impl SecretSpec {
    /// Convert to a [`CloudSecret`] the resolver renders into `CREATE SECRET`.
    pub fn to_cloud_secret(&self) -> CloudSecret {
        CloudSecret::new(self.secret_type.clone(), self.params.clone())
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    fn s3(params: &[(&str, &str)]) -> SecretSpec {
        SecretSpec {
            secret_type: "s3".into(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_string(), SecretString::from((*v).to_string())))
                .collect(),
        }
    }

    /// A connection's secret renders the `CREATE SECRET` the resolver installs,
    /// via `apply_source_creds`.
    #[test]
    fn a_connection_secret_round_trips_into_create_secret() {
        let secret = s3(&[("REGION", "eu-west-1"), ("SECRET", "shhh")]);
        assert_eq!(secret.params["REGION"].expose_secret(), "eu-west-1");
        let sql = fossil_storage::ResolvedPath::with_secret("s3://b", secret.to_cloud_secret())
            .create_secret_sql("s")
            .expect("sql");
        assert!(sql.contains("TYPE s3"), "type missing: {sql}");
        assert!(sql.contains("REGION 'eu-west-1'"), "param missing: {sql}");
    }

    #[test]
    fn secrets_never_appear_in_debug() {
        let creds = ConnectionCreds {
            url: "s3://b".into(),
            secret: Some(s3(&[("SECRET", "leaky-value")])),
        };
        assert!(
            !format!("{creds:?}").contains("leaky-value"),
            "secret leaked through Debug"
        );
    }
}
