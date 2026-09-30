//! [`StorageCredential`] → [`Grant`]: one parse of the vended credential, and
//! every form of access fossil derives from it.

use std::collections::HashMap;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use fossil_graph_schema::{Failure, Foreign, Problem};

use crate::resolved::{CloudSecret, ResolvedPath};

/// A storage credential scoped to one prefix — Iceberg REST's
/// `StorageCredential`, verbatim on the wire.
///
/// `config` carries Iceberg's keys: `s3.access-key-id`, `s3.secret-access-key`,
/// `s3.session-token`, `s3.endpoint`, `s3.path-style-access`, `client.region`,
/// `s3.session-token-expires-at-ms`; or `adls.sas-token.<host>` and
/// `adls.sas-token-expires-at-ms.<host>`, `<host>` being the prefix's
/// `<account>.dfs.core.windows.net`.
#[derive(Clone, Deserialize)]
pub struct StorageCredential {
    /// `s3://bucket/path/` or `abfss://container@account.dfs.core.windows.net/path/`.
    pub prefix: String,
    pub config: HashMap<String, SecretString>,
}

impl std::fmt::Debug for StorageCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys: Vec<&String> = self.config.keys().collect();
        keys.sort();
        f.debug_struct("StorageCredential")
            .field("prefix", &self.prefix)
            .field("config_keys", &keys)
            .finish()
    }
}

/// What a credential was vended for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
}

impl Access {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// Why storage refused. Typed, and a [`Failure`] at the boundary — see the
/// `From` below for the code each variant is.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("a vended prefix names a directory and ends in `/`: {0:?}")]
    Prefix(String),
    /// No kind of store fossil reads routes it: a vended prefix of another
    /// scheme, or a locator no credential covers that is not public `http(s)`.
    #[error(
        "{0:?} has no route: fossil reads s3://bucket/… and \
         abfss://container@account.dfs.core.windows.net/… through a vended credential, and \
         http(s) without one"
    )]
    Store(String),
    #[error("the credential for {prefix} carries no `{key}`")]
    Missing { prefix: String, key: String },
    #[error("`{key}` is not {what}: {value:?}")]
    Malformed {
        prefix: String,
        key: String,
        what: &'static str,
        value: String,
    },
    #[error("{locator} lies outside {prefix}, the prefix the credential was vended for")]
    Outside { locator: String, prefix: String },
    /// The host's own rejection, kept whole.
    #[error("the host refused {scope}: {cause}")]
    HostRefused {
        scope: String,
        #[source]
        cause: Foreign,
    },
    #[error("the host vended no {access} credential for {scope}")]
    NoCredential { scope: String, access: &'static str },
    /// A store answered with an error, kept whole.
    #[cfg(feature = "object-store")]
    #[error("{locator}: {source}")]
    Io {
        locator: String,
        #[source]
        source: Box<object_store::Error>,
    },
}

impl From<StorageError> for Failure {
    fn from(e: StorageError) -> Self {
        let malformed = |prefix: String, key: String, reason: String| {
            Self::new(Problem::MalformedCredential {
                prefix,
                key,
                reason,
            })
        };
        match e {
            StorageError::Prefix(prefix) => malformed(
                prefix,
                "prefix".to_string(),
                "names a directory and must end in `/`".to_string(),
            ),
            StorageError::Store(locator) => Self::new(Problem::NoRoute { locator }),
            StorageError::Missing { prefix, key } => {
                malformed(prefix, key, "is missing".to_string())
            }
            StorageError::Malformed {
                prefix,
                key,
                what,
                value,
            } => malformed(prefix, key, format!("is not {what}: {value:?}")),
            StorageError::Outside { locator, prefix } => {
                Self::new(Problem::OutsidePrefix { locator, prefix })
            }
            StorageError::HostRefused { scope, cause } => {
                Self::new(Problem::HostRefused { scope }).caused_by(cause)
            }
            StorageError::NoCredential { scope, access } => Self::new(Problem::NoCredential {
                scope,
                access: access.to_string(),
            }),
            #[cfg(feature = "object-store")]
            StorageError::Io { locator, source } => {
                Self::new(Problem::Unreachable { locator }).caused_by(*source)
            }
        }
    }
}

/// A parsed [`StorageCredential`].
pub struct Grant {
    prefix: String,
    store: Store,
    expires_at_ms: Option<u64>,
}

pub(crate) enum Store {
    S3(S3),
    Azure {
        account: String,
        container: String,
        sas: SecretString,
    },
}

pub(crate) struct S3 {
    pub(crate) key_id: SecretString,
    pub(crate) secret: SecretString,
    pub(crate) token: Option<SecretString>,
    pub(crate) region: String,
    pub(crate) endpoint: Option<Endpoint>,
    pub(crate) path_style: bool,
}

pub(crate) struct Endpoint {
    pub(crate) authority: String,
    pub(crate) ssl: bool,
}

/// RFC 3986's unreserved set, and `/`: a blob path is encoded segment by segment.
const PATH: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

impl TryFrom<StorageCredential> for Grant {
    type Error = StorageError;

    fn try_from(credential: StorageCredential) -> Result<Self, StorageError> {
        let StorageCredential { prefix, config } = credential;
        if !prefix.ends_with('/') {
            return Err(StorageError::Prefix(prefix));
        }
        let take = |key: &str| config.get(key).map(|v| v.expose_secret().to_string());
        let need = |key: &str| {
            config
                .get(key)
                .cloned()
                .ok_or_else(|| StorageError::Missing {
                    prefix: prefix.clone(),
                    key: key.to_string(),
                })
        };
        let millis = |key: &str| {
            take(key)
                .map(|v| {
                    v.parse::<u64>().map_err(|_| StorageError::Malformed {
                        prefix: prefix.clone(),
                        key: key.to_string(),
                        what: "milliseconds since the epoch",
                        value: v,
                    })
                })
                .transpose()
        };

        if let Some(rest) = prefix.strip_prefix("s3://") {
            rest.split_once('/')
                .filter(|(bucket, _)| !bucket.is_empty())
                .ok_or_else(|| StorageError::Store(prefix.clone()))?;
            let endpoint = take("s3.endpoint")
                .map(|e| endpoint(&prefix, &e))
                .transpose()?;
            let path_style = match take("s3.path-style-access").as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                Some(other) => {
                    return Err(StorageError::Malformed {
                        prefix: prefix.clone(),
                        key: "s3.path-style-access".to_string(),
                        what: "`true` or `false`",
                        value: other.to_string(),
                    });
                }
            };
            let store = Store::S3(S3 {
                key_id: need("s3.access-key-id")?,
                secret: need("s3.secret-access-key")?,
                token: config.get("s3.session-token").cloned(),
                region: need("client.region")?.expose_secret().to_string(),
                endpoint,
                path_style,
            });
            let expires_at_ms = millis("s3.session-token-expires-at-ms")?;
            return Ok(Self {
                prefix,
                store,
                expires_at_ms,
            });
        }

        if let Some(rest) = prefix.strip_prefix("abfss://") {
            let (authority, _) = rest
                .split_once('/')
                .ok_or_else(|| StorageError::Store(prefix.clone()))?;
            let (container, host) = authority
                .split_once('@')
                .ok_or_else(|| StorageError::Store(prefix.clone()))?;
            let account = host
                .strip_suffix(".dfs.core.windows.net")
                .filter(|a| !a.is_empty() && !container.is_empty())
                .ok_or_else(|| StorageError::Store(prefix.clone()))?
                .to_string();
            let store = Store::Azure {
                account,
                container: container.to_string(),
                sas: need(&format!("adls.sas-token.{host}"))?,
            };
            let expires_at_ms = millis(&format!("adls.sas-token-expires-at-ms.{host}"))?;
            return Ok(Self {
                prefix,
                store,
                expires_at_ms,
            });
        }

        Err(StorageError::Store(prefix))
    }
}

fn endpoint(prefix: &str, url: &str) -> Result<Endpoint, StorageError> {
    let malformed = || StorageError::Malformed {
        prefix: prefix.to_string(),
        key: "s3.endpoint".to_string(),
        what: "an http(s) URL with no path",
        value: url.to_string(),
    };
    let (ssl, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(malformed());
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() || authority.contains('/') {
        return Err(malformed());
    }
    Ok(Endpoint {
        authority: authority.to_string(),
        ssl,
    })
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grant")
            .field("prefix", &self.prefix)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

impl Grant {
    /// The prefix the credential was vended for, as the host wrote it.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// When the credential stops working; `None` when the host did not say.
    #[must_use]
    pub const fn expires_at_ms(&self) -> Option<u64> {
        self.expires_at_ms
    }

    #[cfg(feature = "object-store")]
    pub(crate) const fn store(&self) -> &Store {
        &self.store
    }

    /// The secret's name for `access` on this prefix — the same on every renewal,
    /// so `CREATE OR REPLACE` rotates it under a view that stays.
    #[must_use]
    pub fn secret_name(&self, access: Access) -> String {
        // FNV-1a: stable across builds and targets, which `DefaultHasher` is not.
        let hash = self.prefix.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        format!("fossil_{}_{hash:016x}", access.as_str())
    }

    /// `CREATE OR REPLACE SECRET` scoped to the prefix, for a store `DuckDB`
    /// reaches with a secret; `None` for Azure, which is lent file by file.
    #[must_use]
    pub fn install_sql(&self, access: Access) -> Option<String> {
        let Store::S3(S3 {
            key_id,
            secret,
            token,
            region,
            endpoint,
            path_style,
            ..
        }) = &self.store
        else {
            return None;
        };
        let mut params: HashMap<String, SecretString> = HashMap::from([
            ("KEY_ID".to_string(), key_id.clone()),
            ("SECRET".to_string(), secret.clone()),
            ("REGION".to_string(), SecretString::from(region.clone())),
            (
                "URL_STYLE".to_string(),
                SecretString::from(if *path_style { "path" } else { "vhost" }),
            ),
        ]);
        if let Some(token) = token {
            params.insert("SESSION_TOKEN".to_string(), token.clone());
        }
        if let Some(Endpoint { authority, ssl }) = endpoint {
            params.insert(
                "ENDPOINT".to_string(),
                SecretString::from(authority.clone()),
            );
            params.insert("USE_SSL".to_string(), SecretString::from(ssl.to_string()));
        }
        ResolvedPath::with_secret(&self.prefix, CloudSecret::new("s3", params))
            .create_secret_sql(&self.secret_name(access))
    }

    /// The statement that takes [`Self::install_sql`] back.
    #[must_use]
    pub fn uninstall_sql(&self, access: Access) -> Option<String> {
        matches!(self.store, Store::S3(_))
            .then(|| format!("DROP SECRET IF EXISTS {}", self.secret_name(access)))
    }

    /// What SQL calls `locator`: the locator itself where a secret covers it,
    /// and a scheme-less name the engine lends where none can.
    ///
    /// # Errors
    /// [`StorageError::Outside`] when `locator` is not under the prefix — a
    /// read that no secret covers would go out anonymously.
    pub fn name(&self, locator: &str) -> Result<String, StorageError> {
        let rest = self.within(locator)?;
        Ok(match &self.store {
            Store::S3(_) => locator.to_string(),
            Store::Azure {
                account, container, ..
            } => format!("azure/{account}/{container}/{}", self.path_of(rest)),
        })
    }

    /// The URL the engine lends [`Self::name`] to; `None` where the name is
    /// already readable.
    ///
    /// # Errors
    /// As [`Self::name`].
    pub fn lend(&self, locator: &str) -> Result<Option<String>, StorageError> {
        let rest = self.within(locator)?;
        Ok(match &self.store {
            Store::S3(_) => None,
            Store::Azure { .. } => Some(self.azure_url(rest)),
        })
    }

    fn within<'l>(&self, locator: &'l str) -> Result<&'l str, StorageError> {
        locator
            .strip_prefix(&self.prefix)
            .ok_or_else(|| StorageError::Outside {
                locator: locator.to_string(),
                prefix: self.prefix.clone(),
            })
    }

    /// The object key (S3) or blob path (Azure) of what follows the prefix.
    fn path_of(&self, rest: &str) -> String {
        let authority_end = self.prefix.find("://").map_or(0, |i| i + 3);
        let base = self.prefix[authority_end..]
            .split_once('/')
            .map_or("", |(_, path)| path);
        format!("{base}{rest}")
    }

    fn azure_url(&self, rest: &str) -> String {
        let Store::Azure {
            account,
            container,
            sas,
        } = &self.store
        else {
            unreachable!("only an Azure grant lends");
        };
        let path = utf8_percent_encode(&self.path_of(rest), PATH).to_string();
        let sas = sas.expose_secret().trim_start_matches('?');
        format!("https://{account}.blob.core.windows.net/{container}/{path}?{sas}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(prefix: &str, config: &[(&str, &str)]) -> StorageCredential {
        StorageCredential {
            prefix: prefix.to_string(),
            config: config
                .iter()
                .map(|(k, v)| ((*k).to_string(), SecretString::from(*v)))
                .collect(),
        }
    }

    fn minio() -> Grant {
        Grant::try_from(credential(
            "s3://keasy-dev/output/job-1/",
            &[
                ("s3.access-key-id", "KEY"),
                ("s3.secret-access-key", "SEC'RET"),
                ("s3.session-token", "TOKEN"),
                ("s3.endpoint", "http://localhost:9000"),
                ("s3.path-style-access", "true"),
                ("client.region", "us-east-1"),
                ("s3.session-token-expires-at-ms", "1790000000000"),
            ],
        ))
        .expect("grant")
    }

    fn azure() -> Grant {
        Grant::try_from(credential(
            "abfss://lake@acct.dfs.core.windows.net/raw/",
            &[
                (
                    "adls.sas-token.acct.dfs.core.windows.net",
                    "sv=2025&sr=d&sig=a%2Bb",
                ),
                (
                    "adls.sas-token-expires-at-ms.acct.dfs.core.windows.net",
                    "42",
                ),
            ],
        ))
        .expect("grant")
    }

    #[test]
    fn an_s3_credential_is_a_scoped_secret_with_its_quotes_escaped() {
        let grant = minio();
        assert_eq!(grant.expires_at_ms(), Some(1_790_000_000_000));
        assert_eq!(
            grant.install_sql(Access::Read).expect("s3 installs"),
            "CREATE OR REPLACE SECRET fossil_read_6ea31fe7018ba8d9 (TYPE s3, ENDPOINT 'localhost:9000', \
             KEY_ID 'KEY', REGION 'us-east-1', SECRET 'SEC''RET', SESSION_TOKEN 'TOKEN', \
             URL_STYLE 'path', USE_SSL 'false', SCOPE 's3://keasy-dev/output/job-1/')"
        );
        assert_eq!(
            grant.uninstall_sql(Access::Read).as_deref(),
            Some("DROP SECRET IF EXISTS fossil_read_6ea31fe7018ba8d9")
        );
    }

    #[test]
    fn read_and_write_on_one_prefix_are_two_secrets() {
        let grant = minio();
        assert_ne!(
            grant.secret_name(Access::Read),
            grant.secret_name(Access::Write)
        );
    }

    #[test]
    fn aws_without_an_endpoint_is_virtual_hosted_over_tls() {
        let grant = Grant::try_from(credential(
            "s3://b/p/",
            &[
                ("s3.access-key-id", "K"),
                ("s3.secret-access-key", "S"),
                ("client.region", "eu-west-1"),
            ],
        ))
        .expect("grant");
        let sql = grant.install_sql(Access::Read).expect("sql");
        assert!(
            sql.contains("URL_STYLE 'vhost'") && !sql.contains("ENDPOINT"),
            "{sql}"
        );
        assert!(!sql.contains("SESSION_TOKEN"), "{sql}");
    }

    #[test]
    fn nothing_outside_the_prefix_is_named_or_lent() {
        let outside = "s3://keasy-dev/output/job-1-evil/x";
        for result in [
            minio().name(outside).map(drop),
            minio().lend(outside).map(drop),
        ] {
            assert!(
                matches!(result, Err(StorageError::Outside { .. })),
                "{result:?}"
            );
        }
    }

    #[test]
    fn an_azure_credential_lends_each_file_by_a_schemeless_name() {
        let grant = azure();
        assert_eq!(grant.expires_at_ms(), Some(42));
        assert_eq!(grant.install_sql(Access::Read), None);
        assert_eq!(grant.uninstall_sql(Access::Read), None);
        let locator = "abfss://lake@acct.dfs.core.windows.net/raw/a b.csv";
        assert_eq!(
            grant.name(locator).expect("name"),
            "azure/acct/lake/raw/a b.csv"
        );
        assert_eq!(
            grant.lend(locator).expect("lend").as_deref(),
            Some("https://acct.blob.core.windows.net/lake/raw/a%20b.csv?sv=2025&sr=d&sig=a%2Bb")
        );
    }

    #[test]
    fn what_is_not_a_vended_credential_is_refused() {
        let err = |prefix: &str, config: &[(&str, &str)]| {
            Grant::try_from(credential(prefix, config)).expect_err("refused")
        };
        assert!(matches!(err("s3://b/p", &[]), StorageError::Prefix(_)));
        assert!(matches!(err("gs://b/p/", &[]), StorageError::Store(_)));
        assert!(matches!(err("az://c/p/", &[]), StorageError::Store(_)));
        assert!(matches!(
            err("s3://b/p/", &[]),
            StorageError::Missing { .. }
        ));
        assert!(matches!(
            err(
                "abfss://c@acct.dfs.core.windows.net/p/",
                &[("adls.sas-token.other", "x")]
            ),
            StorageError::Missing { .. }
        ));
        let s3 = [
            ("s3.access-key-id", "K"),
            ("s3.secret-access-key", "S"),
            ("client.region", "r"),
        ];
        let with = |extra: (&str, &str)| {
            let mut c = s3.to_vec();
            c.push(extra);
            err("s3://b/p/", &c)
        };
        assert!(matches!(
            with(("s3.endpoint", "localhost:9000")),
            StorageError::Malformed { .. }
        ));
        assert!(matches!(
            with(("s3.path-style-access", "yes")),
            StorageError::Malformed { .. }
        ));
        assert!(matches!(
            with(("s3.session-token-expires-at-ms", "soon")),
            StorageError::Malformed { .. }
        ));
    }

    #[test]
    fn the_wire_form_is_icebergs() {
        let credential: StorageCredential = serde_json::from_str(
            r#"{"prefix":"s3://b/p/","config":{"s3.access-key-id":"K","s3.secret-access-key":"S","client.region":"r"}}"#,
        )
        .expect("json");
        let debug = format!("{credential:?}");
        assert!(
            debug.contains("s3.access-key-id") && !debug.contains("\"K\""),
            "{debug}"
        );
        assert!(Grant::try_from(credential).is_ok());
    }
}
