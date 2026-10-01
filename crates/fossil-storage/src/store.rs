//! [`Storage`] — vended credentials as `object_store` stores, which is how
//! `DataFusion` reads and fossil writes.
//!
//! One store per credential, built from the parsed [`Grant`] and renewed
//! through a [`CredentialProvider`] at `expires − 5 min`, the margin Iceberg's
//! `VendedCredentialsProvider` uses. `DataFusion` looks a store up by
//! `scheme://authority`, and two connections can vend two prefixes of one
//! bucket, so each authority gets a [`Routed`] store that sends a path to the
//! credential with the longest prefix covering it — Iceberg's rule for several
//! storage credentials — and refuses a path none covers before any request
//! leaves.
//!
//! A locator no connection names is read only when it is a public `http(s)` URL,
//! through an [`HttpStore`] with no credential.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::stream::{self, BoxStream, StreamExt, TryStreamExt};
use object_store::aws::{AmazonS3Builder, AwsCredential};
use object_store::azure::{AzureCredential, MicrosoftAzureBuilder};
use object_store::http::HttpBuilder;
use object_store::path::Path;
use object_store::{
    BackoffConfig, ClientOptions, CopyOptions, CredentialProvider, GetOptions, GetResult,
    ListResult, MultipartUpload, ObjectMeta, ObjectStore, ObjectStoreExt, PutMultipartOptions,
    PutOptions, PutPayload, PutResult, RenameOptions, RetryConfig,
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};

use fossil_graph_schema::Foreign;

use crate::credential::{Access, Endpoint, Grant, S3, StorageCredential, StorageError, Store};

/// Renew this long before a credential expires.
const RENEW_BEFORE_MS: u64 = 5 * 60_000;
/// A body larger than this is uploaded in parts of this size; S3's floor is 5 MiB.
const PART_BYTES: usize = 8 * 1024 * 1024;
/// Parts in flight at once.
const PARTS_IN_FLIGHT: usize = 4;
/// How long a store may take to accept a connection — `/docs/design/failure`'s G1 table.
const CONNECT: Duration = Duration::from_secs(5);
/// How long one request may take, whole.
const REQUEST: Duration = Duration::from_secs(30);
/// How many times a request that failed in a way worth repeating is sent again.
const RETRIES: usize = 2;

/// The client options every store fossil builds is given: the figures in
/// `/docs/design/failure`.
///
/// **On wasm32 none of them reach the request.** `object_store` builds its
/// wasm32 `reqwest` client without the timeouts — `fetch` has no connect phase
/// and no timeout of its own — so [`Routed`] races every request against
/// [`REQUEST`] there instead. Retries are off on wasm32 for a worse reason:
/// `object_store`'s backoff sleeps with `tokio::time::sleep`, which panics
/// outside a Tokio runtime, and a page has none.
fn client() -> ClientOptions {
    ClientOptions::new()
        .with_connect_timeout(CONNECT)
        .with_timeout(REQUEST)
}

fn retry() -> RetryConfig {
    RetryConfig {
        backoff: BackoffConfig::default(),
        max_retries: if cfg!(target_arch = "wasm32") {
            0
        } else {
            RETRIES
        },
        retry_timeout: REQUEST * 3,
    }
}

/// Why a host did not answer with what it was asked for.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The host's own rejection, kept whole.
    #[error(transparent)]
    Refused(#[from] Foreign),
    /// No answer within `after` milliseconds.
    #[error("no answer within {after} ms")]
    Silent { after: u64 },
}

impl HostError {
    /// The storage failure this is, for a request about `scope`.
    fn about(self, scope: String) -> StorageError {
        match self {
            Self::Refused(cause) => StorageError::HostRefused { scope, cause },
            Self::Silent { after } => StorageError::HostSilent { scope, after },
        }
    }
}

/// What a credential is asked for: a connection's prefix, or a job's dataset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Connection(String),
    Job(String),
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(name) => write!(f, "connection {name}"),
            Self::Job(id) => write!(f, "job {id}"),
        }
    }
}

/// What a host gives fossil to reach storage — `@fossil-lang/types`' `Host`.
pub trait Host: fmt::Debug + Send + Sync {
    /// Connection name → canonical prefix, as `@name/…` expands against it.
    ///
    /// A refusal is the host's own error, kept whole as a [`Foreign`]; a host
    /// that answers nothing within its deadline is [`HostError::Silent`].
    fn connections(&self) -> BoxFuture<'static, Result<HashMap<String, String>, HostError>>;
    /// Vend `access` on `scope`: short-lived, scoped to a prefix.
    fn credentials(
        &self,
        scope: &Scope,
        access: Access,
    ) -> BoxFuture<'static, Result<Vec<StorageCredential>, HostError>>;
}

/// Every store a run reaches, keyed as `DataFusion` registers them.
#[derive(Debug)]
pub struct Storage {
    host: Arc<dyn Host>,
    authorities: HashMap<String, Routed>,
    granted: HashMap<(Scope, Access), Vec<String>>,
}

impl Storage {
    #[must_use]
    pub fn new(host: Arc<dyn Host>) -> Self {
        Self {
            host,
            authorities: HashMap::new(),
            granted: HashMap::new(),
        }
    }

    /// Vend `access` on `scope`, route every prefix it covers, and answer the
    /// prefixes. Asking twice for the same pair is one request.
    ///
    /// # Errors
    /// The host refuses, vends nothing, or vends a credential fossil does not read.
    pub async fn grant(&mut self, scope: Scope, access: Access) -> Result<&[String], StorageError> {
        let key = (scope, access);
        if self.granted.contains_key(&key) {
            return Ok(&self.granted[&key]);
        }
        let (scope, access) = key;
        let credentials = self
            .host
            .credentials(&scope, access)
            .await
            .map_err(|e| e.about(scope.to_string()))?;
        if credentials.is_empty() {
            return Err(StorageError::NoCredential {
                scope: scope.to_string(),
                access: access.as_str(),
            });
        }
        let mut prefixes = Vec::with_capacity(credentials.len());
        for credential in credentials {
            let grant = Grant::try_from(credential)?;
            let (authority, key) = split(grant.prefix())?;
            let store = build(&grant, Renewal::new(&self.host, &scope, access, &grant))?;
            self.authorities
                .entry(authority)
                .or_default()
                .add(key, store);
            prefixes.push(grant.prefix().to_string());
        }
        Ok(self.granted.entry((scope, access)).or_insert(prefixes))
    }

    /// The host's connections.
    ///
    /// # Errors
    /// The host could not answer.
    pub async fn connections(&self) -> Result<HashMap<String, String>, StorageError> {
        self.host
            .connections()
            .await
            .map_err(|e| e.about("its connections".to_string()))
    }

    /// Whether a store is routed for `locator` already.
    #[must_use]
    pub fn covers(&self, locator: &str) -> bool {
        self.resolve(locator).is_ok()
    }

    /// Route a public `http(s)` locator — one no credential covers — through a
    /// store that sends no credential.
    ///
    /// # Errors
    /// `locator` is not an `http(s)` URL.
    pub fn public(&mut self, locator: &str) -> Result<(), StorageError> {
        let (authority, _) = split(locator)?;
        if !(authority.starts_with("https://") || authority.starts_with("http://")) {
            return Err(StorageError::Store(locator.to_string()));
        }
        let routed = self.authorities.entry(authority.clone()).or_default();
        if routed.routes.iter().all(|r| !r.key.is_empty()) {
            let store = HttpBuilder::new()
                .with_url(&authority)
                .with_client_options(client())
                .with_retry(retry())
                .build()
                .map_err(|e| io(locator, e))?;
            routed.add(String::new(), Arc::new(store));
        }
        Ok(())
    }

    /// Route `prefix` through a store the host already holds — a native host's
    /// filesystem, a test's memory — with no credential to vend.
    ///
    /// # Errors
    /// `prefix` is not `scheme://authority/…` ending in `/`.
    pub fn with_store(
        &mut self,
        prefix: &str,
        store: Arc<dyn ObjectStore>,
    ) -> Result<(), StorageError> {
        if !prefix.ends_with('/') {
            return Err(StorageError::Prefix(prefix.to_string()));
        }
        let (authority, key) = split(prefix)?;
        self.authorities
            .entry(authority)
            .or_default()
            .add(key, store);
        Ok(())
    }

    /// `(scheme://authority, store)` for each authority, for `DataFusion` to register.
    pub fn stores(&self) -> impl Iterator<Item = (&str, Arc<dyn ObjectStore>)> {
        self.authorities
            .iter()
            .map(|(authority, routed)| (authority.as_str(), Arc::new(routed.clone()) as _))
    }

    /// The bytes at `locator`.
    ///
    /// # Errors
    /// Nothing granted covers `locator`, or the store answers with an error.
    pub async fn get(&self, locator: &str) -> Result<Bytes, StorageError> {
        let (store, path) = self.resolve(locator)?;
        bounded(async move { store.get(&path).await?.bytes().await })
            .await
            .map_err(|e| io(locator, e))
    }

    /// Write `bytes` at `locator`, in parts when it is large.
    ///
    /// # Errors
    /// Nothing granted covers `locator`, or the store answers with an error.
    pub async fn put(&self, locator: &str, bytes: Bytes) -> Result<(), StorageError> {
        let (store, path) = self.resolve(locator)?;
        if bytes.len() <= PART_BYTES {
            bounded(store.put(&path, PutPayload::from(bytes)))
                .await
                .map_err(|e| io(locator, e))?;
            return Ok(());
        }
        let mut upload = bounded(store.put_multipart(&path))
            .await
            .map_err(|e| io(locator, e))?;
        let parts: Vec<_> = bytes
            .chunks(PART_BYTES)
            .map(|chunk| bounded(upload.put_part(PutPayload::from(bytes.slice_ref(chunk)))))
            .collect();
        let sent = stream::iter(parts)
            .buffer_unordered(PARTS_IN_FLIGHT)
            .try_collect::<Vec<()>>()
            .await;
        if let Err(e) = sent {
            return Err(match upload.abort().await {
                Ok(()) => io(locator, e),
                Err(abort) => StorageError::Orphaned {
                    locator: locator.to_string(),
                    source: Box::new(e),
                    abort: Box::new(abort),
                },
            });
        }
        bounded(upload.complete())
            .await
            .map_err(|e| io(locator, e))?;
        Ok(())
    }

    fn resolve(&self, locator: &str) -> Result<(Arc<dyn ObjectStore>, Path), StorageError> {
        let (authority, key) = split(locator)?;
        let path = Path::from(key.as_str());
        self.authorities
            .get(&authority)
            .and_then(|routed| routed.covering(key.as_str()))
            .map(|store| (Arc::clone(store), path))
            .ok_or_else(|| StorageError::Outside {
                locator: locator.to_string(),
                prefix: "any prefix the host vended".to_string(),
            })
    }
}

fn io(locator: &str, e: object_store::Error) -> StorageError {
    StorageError::Io {
        locator: locator.to_string(),
        source: Box::new(e),
    }
}

/// `work`, given at most [`REQUEST`] on wasm32, where nothing below bounds it —
/// see [`client`]. Elsewhere the client's own timeout does, and this is `work`.
///
/// It bounds what a call awaits: a listing or a body read as a stream is
/// bounded until it starts, not until it ends.
async fn bounded<T>(
    work: impl Future<Output = object_store::Result<T>>,
) -> object_store::Result<T> {
    #[cfg(all(target_arch = "wasm32", feature = "js"))]
    {
        use futures::future::{Either, select};
        let work = std::pin::pin!(work);
        let after = u64::try_from(REQUEST.as_millis()).unwrap_or(u64::MAX);
        match select(work, crate::js::elapse(after)).await {
            Either::Left((done, _)) => done,
            Either::Right(((), _)) => Err(object_store::Error::Generic {
                store: "fossil-storage",
                source: format!("the store did not answer within {after} ms").into(),
            }),
        }
    }
    #[cfg(not(all(target_arch = "wasm32", feature = "js")))]
    {
        work.await
    }
}

/// `scheme://authority/key` → (`scheme://authority`, `key`), the split
/// `DataFusion` registers stores by.
fn split(url: &str) -> Result<(String, String), StorageError> {
    let store = || StorageError::Store(url.to_string());
    let (scheme, rest) = url.split_once("://").ok_or_else(store)?;
    let (authority, key) = rest.split_once('/').unwrap_or((rest, ""));
    if scheme.is_empty() || authority.is_empty() {
        return Err(store());
    }
    Ok((format!("{scheme}://{authority}"), key.to_string()))
}

fn build(grant: &Grant, renewal: Renewal) -> Result<Arc<dyn ObjectStore>, StorageError> {
    let built = match grant.store() {
        Store::S3(S3 {
            region,
            endpoint,
            path_style,
            ..
        }) => {
            // The bucket is the prefix's authority, and `with_url` reads it from there.
            let mut builder = AmazonS3Builder::new()
                .with_url(grant.prefix())
                .with_region(region)
                .with_virtual_hosted_style_request(!path_style)
                .with_client_options(client())
                .with_retry(retry())
                .with_credentials(Arc::new(Vended::<AwsCredential>::new(grant, renewal)));
            if let Some(Endpoint { authority, ssl }) = endpoint {
                let scheme = if *ssl { "https" } else { "http" };
                builder = builder
                    .with_endpoint(format!("{scheme}://{authority}"))
                    .with_allow_http(!ssl);
            }
            builder.build().map(|s| Arc::new(s) as _)
        }
        Store::Azure {
            account, container, ..
        } => MicrosoftAzureBuilder::new()
            .with_account(account)
            .with_container_name(container)
            .with_client_options(client())
            .with_retry(retry())
            .with_credentials(Arc::new(Vended::<AzureCredential>::new(grant, renewal)))
            .build()
            .map(|s| Arc::new(s) as _),
    };
    built.map_err(|e| io(grant.prefix(), e))
}

/// The prefixes of one authority, each with the store of the credential vended
/// for it.
#[derive(Debug, Clone, Default)]
struct Routed {
    routes: Vec<Route>,
}

#[derive(Debug, Clone)]
struct Route {
    /// The object key the prefix names, ending in `/`, or empty for all of it.
    key: String,
    store: Arc<dyn ObjectStore>,
}

impl Routed {
    fn add(&mut self, key: String, store: Arc<dyn ObjectStore>) {
        self.routes.retain(|r| r.key != key);
        self.routes.push(Route { key, store });
    }

    /// The store of the longest prefix covering `key`. A directory is covered
    /// by its own prefix, which is how a listing of `output/job` reaches
    /// `output/job/`.
    fn covering(&self, key: &str) -> Option<&Arc<dyn ObjectStore>> {
        self.routes
            .iter()
            .filter(|r| key.starts_with(&r.key) || format!("{key}/") == r.key)
            .max_by_key(|r| r.key.len())
            .map(|r| &r.store)
    }

    fn store(&self, location: &Path) -> object_store::Result<&Arc<dyn ObjectStore>> {
        self.covering(location.as_ref())
            .ok_or_else(|| object_store::Error::PermissionDenied {
                path: location.to_string(),
                source: "no credential the host vended covers it".into(),
            })
    }
}

impl fmt::Display for Routed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<&str> = self.routes.iter().map(|r| r.key.as_str()).collect();
        write!(f, "Routed({})", keys.join(", "))
    }
}

#[async_trait]
#[deny(clippy::missing_trait_methods)]
impl ObjectStore for Routed {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        bounded(self.store(location)?.put_opts(location, payload, opts)).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        bounded(self.store(location)?.put_multipart_opts(location, opts)).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        bounded(self.store(location)?.get_opts(location, options)).await
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> object_store::Result<Vec<Bytes>> {
        bounded(self.store(location)?.get_ranges(location, ranges)).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        let routed = self.clone();
        locations
            .and_then(move |location| {
                let store = routed.store(&location).cloned();
                async move {
                    store?.delete(&location).await?;
                    Ok(location)
                }
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        let prefix = prefix.cloned().unwrap_or_default();
        match self.store(&prefix) {
            Ok(store) => store.list(Some(&prefix)),
            Err(e) => stream::once(async { Err(e) }).boxed(),
        }
    }

    fn list_with_offset(
        &self,
        prefix: Option<&Path>,
        offset: &Path,
    ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        let prefix = prefix.cloned().unwrap_or_default();
        match self.store(&prefix) {
            Ok(store) => store.list_with_offset(Some(&prefix), offset),
            Err(e) => stream::once(async { Err(e) }).boxed(),
        }
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        let prefix = prefix.cloned().unwrap_or_default();
        bounded(self.store(&prefix)?.list_with_delimiter(Some(&prefix))).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        bounded(self.same(from, to)?.copy_opts(from, to, options)).await
    }

    async fn rename_opts(
        &self,
        from: &Path,
        to: &Path,
        options: RenameOptions,
    ) -> object_store::Result<()> {
        bounded(self.same(from, to)?.rename_opts(from, to, options)).await
    }
}

impl Routed {
    /// One store for both ends, or the move would cross two credentials.
    fn same(&self, from: &Path, to: &Path) -> object_store::Result<&Arc<dyn ObjectStore>> {
        let store = self.store(from)?;
        if !Arc::ptr_eq(store, self.store(to)?) {
            return Err(object_store::Error::NotSupported {
                source: format!("{from} and {to} lie under two credentials").into(),
            });
        }
        Ok(store)
    }
}

/// Where a store's credential comes from again when it is about to expire.
#[derive(Debug, Clone)]
struct Renewal {
    host: Arc<dyn Host>,
    scope: Scope,
    access: Access,
    prefix: String,
}

impl Renewal {
    fn new(host: &Arc<dyn Host>, scope: &Scope, access: Access, grant: &Grant) -> Self {
        Self {
            host: Arc::clone(host),
            scope: scope.clone(),
            access,
            prefix: grant.prefix().to_string(),
        }
    }

    async fn renew(&self) -> Result<Grant, StorageError> {
        let fresh = self
            .host
            .credentials(&self.scope, self.access)
            .await
            .map_err(|e| e.about(self.scope.to_string()))?
            .into_iter()
            .find(|c| c.prefix == self.prefix)
            .ok_or_else(|| StorageError::NoCredential {
                scope: format!("{} on {}", self.scope, self.prefix),
                access: self.access.as_str(),
            })?;
        Grant::try_from(fresh)
    }
}

/// The credential a [`Grant`] carries, in the form one store signs with.
trait FromGrant: Sized {
    fn from_grant(grant: &Grant) -> Option<Self>;
}

impl FromGrant for AwsCredential {
    fn from_grant(grant: &Grant) -> Option<Self> {
        let Store::S3(S3 {
            key_id,
            secret,
            token,
            ..
        }) = grant.store()
        else {
            return None;
        };
        Some(Self {
            key_id: key_id.expose_secret().to_string(),
            secret_key: secret.expose_secret().to_string(),
            token: token.as_ref().map(|t| t.expose_secret().to_string()),
        })
    }
}

impl FromGrant for AzureCredential {
    fn from_grant(grant: &Grant) -> Option<Self> {
        let Store::Azure { sas, .. } = grant.store() else {
            return None;
        };
        let decoded = percent_encoding::percent_decode_str(sas.expose_secret())
            .decode_utf8_lossy()
            .into_owned();
        let pairs = decoded
            .trim_start_matches('?')
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Some(Self::SASToken(pairs))
    }
}

/// A store's credential, renewed from the host before it expires.
struct Vended<C> {
    renewal: Renewal,
    current: Mutex<Current<C>>,
}

struct Current<C> {
    credential: Arc<C>,
    expires_at_ms: Option<u64>,
}

impl<C: FromGrant> Vended<C> {
    fn new(grant: &Grant, renewal: Renewal) -> Self {
        let credential = C::from_grant(grant).expect("a store is built for its own kind of grant");
        Self {
            renewal,
            current: Mutex::new(Current {
                credential: Arc::new(credential),
                expires_at_ms: grant.expires_at_ms(),
            }),
        }
    }
}

impl<C> fmt::Debug for Vended<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vended")
            .field("prefix", &self.renewal.prefix)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl<C: FromGrant + Send + Sync + 'static> CredentialProvider for Vended<C> {
    type Credential = C;

    async fn get_credential(&self) -> object_store::Result<Arc<C>> {
        let mut current = self.current.lock().await;
        let due = current
            .expires_at_ms
            .is_some_and(|at| now_ms() + RENEW_BEFORE_MS >= at);
        if due {
            match self.renewal.renew().await {
                Ok(grant) => {
                    if let Some(credential) = C::from_grant(&grant) {
                        current.credential = Arc::new(credential);
                        current.expires_at_ms = grant.expires_at_ms();
                    }
                }
                // A renewal that fails while the credential still works is
                // tried again on the next request.
                Err(reason) if current.expires_at_ms.is_some_and(|at| at <= now_ms()) => {
                    return Err(object_store::Error::Generic {
                        store: "fossil-storage",
                        source: Box::new(reason),
                    });
                }
                Err(_) => {}
            }
        }
        Ok(Arc::clone(&current.credential))
    }
}

fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use object_store::memory::InMemory;

    use super::*;

    #[derive(Debug, Default)]
    struct Fake {
        calls: AtomicUsize,
    }

    impl Host for Fake {
        fn connections(&self) -> BoxFuture<'static, Result<HashMap<String, String>, HostError>> {
            Box::pin(async { Ok(HashMap::new()) })
        }

        fn credentials(
            &self,
            scope: &Scope,
            _access: Access,
        ) -> BoxFuture<'static, Result<Vec<StorageCredential>, HostError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let prefix = match scope {
                Scope::Connection(c) => format!("s3://lake/{c}/"),
                Scope::Job(j) => format!("s3://lake/output/{j}/"),
            };
            let credential = StorageCredential {
                prefix,
                config: [
                    ("s3.access-key-id", "K"),
                    ("s3.secret-access-key", "S"),
                    ("client.region", "us-east-1"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into()))
                .collect(),
            };
            Box::pin(async move { Ok(vec![credential]) })
        }
    }

    fn memory() -> Arc<dyn ObjectStore> {
        Arc::new(InMemory::new())
    }

    #[test]
    fn the_longest_prefix_covering_a_key_routes_it() {
        let (data, vocab, root) = (memory(), memory(), memory());
        let mut routed = Routed::default();
        routed.add("data/".into(), Arc::clone(&data));
        routed.add("data/vocab/".into(), Arc::clone(&vocab));
        assert!(Arc::ptr_eq(routed.covering("data/a.csv").unwrap(), &data));
        assert!(Arc::ptr_eq(
            routed.covering("data/vocab/x.shex").unwrap(),
            &vocab
        ));
        assert!(Arc::ptr_eq(routed.covering("data/vocab").unwrap(), &vocab));
        assert!(routed.covering("datalake/a.csv").is_none());
        routed.add(String::new(), Arc::clone(&root));
        assert!(Arc::ptr_eq(
            routed.covering("datalake/a.csv").unwrap(),
            &root
        ));
    }

    #[tokio::test]
    async fn a_path_no_credential_covers_is_refused_before_any_request() {
        let mut routed = Routed::default();
        routed.add("output/job-1/".into(), memory());
        let err = routed
            .put(&Path::from("output/job-2/x"), PutPayload::from_static(b"x"))
            .await
            .expect_err("refused");
        assert!(
            matches!(err, object_store::Error::PermissionDenied { .. }),
            "{err}"
        );
        let err = routed
            .copy(&Path::from("output/job-1/x"), &Path::from("output/job-2/x"))
            .await
            .expect_err("refused");
        assert!(
            matches!(err, object_store::Error::PermissionDenied { .. }),
            "{err}"
        );
    }

    #[tokio::test]
    async fn two_connections_on_one_bucket_are_one_authority_and_one_request_each() {
        let host = Arc::new(Fake::default());
        let mut storage = Storage::new(Arc::clone(&host) as _);
        storage
            .grant(Scope::Connection("data".into()), Access::Read)
            .await
            .expect("grant");
        storage
            .grant(Scope::Connection("vocab".into()), Access::Read)
            .await
            .expect("grant");
        let prefixes = storage
            .grant(Scope::Connection("data".into()), Access::Read)
            .await
            .expect("grant");
        assert_eq!(prefixes, ["s3://lake/data/"]);
        assert_eq!(host.calls.load(Ordering::SeqCst), 2);
        let authorities: Vec<&str> = storage.stores().map(|(a, _)| a).collect();
        assert_eq!(authorities, ["s3://lake"]);
        assert!(matches!(
            storage.resolve("s3://lake/other/x"),
            Err(StorageError::Outside { .. })
        ));
        assert!(storage.resolve("s3://lake/vocab/x.shex").is_ok());
    }

    #[test]
    fn only_a_public_url_is_routed_without_a_credential() {
        let mut storage = Storage::new(Arc::new(Fake::default()));
        storage
            .public("https://example.org/data/a.csv")
            .expect("public");
        assert!(storage.resolve("https://example.org/other.csv").is_ok());
        assert!(matches!(
            storage.public("s3://lake/a.csv"),
            Err(StorageError::Store(_))
        ));
    }

    #[derive(Debug)]
    struct Silent;

    impl Host for Silent {
        fn connections(&self) -> BoxFuture<'static, Result<HashMap<String, String>, HostError>> {
            Box::pin(async { Err(HostError::Silent { after: 30_000 }) })
        }

        fn credentials(
            &self,
            _: &Scope,
            _: Access,
        ) -> BoxFuture<'static, Result<Vec<StorageCredential>, HostError>> {
            Box::pin(async { Err(HostError::Silent { after: 30_000 }) })
        }
    }

    #[tokio::test]
    async fn a_host_that_does_not_answer_is_silent_and_not_refused() {
        let mut storage = Storage::new(Arc::new(Silent));
        let err = storage
            .grant(Scope::Job("j-1".into()), Access::Read)
            .await
            .expect_err("silent");
        let failure = fossil_graph_schema::Failure::from(err);
        assert_eq!(
            failure.problem,
            fossil_graph_schema::Problem::HostSilent {
                scope: "job j-1".into(),
                after: 30_000
            }
        );
        let err = storage.connections().await.expect_err("silent");
        assert!(matches!(
            err,
            StorageError::HostSilent { after: 30_000, .. }
        ));
    }

    /// A store whose multipart upload refuses every part, and refuses to abort.
    #[derive(Debug)]
    struct Unabortable(InMemory);

    impl fmt::Display for Unabortable {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Unabortable")
        }
    }

    #[derive(Debug)]
    struct Refusing;

    #[async_trait]
    impl MultipartUpload for Refusing {
        fn put_part(&mut self, _: PutPayload) -> object_store::UploadPart {
            Box::pin(async {
                Err(object_store::Error::Generic {
                    store: "test",
                    source: "the part was refused".into(),
                })
            })
        }

        async fn complete(&mut self) -> object_store::Result<PutResult> {
            unreachable!("no part was accepted")
        }

        async fn abort(&mut self) -> object_store::Result<()> {
            Err(object_store::Error::Generic {
                store: "test",
                source: "the abort was refused".into(),
            })
        }
    }

    #[async_trait]
    impl ObjectStore for Unabortable {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            opts: PutOptions,
        ) -> object_store::Result<PutResult> {
            self.0.put_opts(location, payload, opts).await
        }

        async fn put_multipart_opts(
            &self,
            _: &Path,
            _: PutMultipartOptions,
        ) -> object_store::Result<Box<dyn MultipartUpload>> {
            Ok(Box::new(Refusing))
        }

        async fn get_opts(
            &self,
            location: &Path,
            options: GetOptions,
        ) -> object_store::Result<GetResult> {
            self.0.get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: BoxStream<'static, object_store::Result<Path>>,
        ) -> BoxStream<'static, object_store::Result<Path>> {
            self.0.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
            self.0.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<ListResult> {
            self.0.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: CopyOptions,
        ) -> object_store::Result<()> {
            self.0.copy_opts(from, to, options).await
        }
    }

    #[tokio::test]
    async fn an_upload_that_could_not_be_aborted_says_so_beside_the_failure() {
        let mut storage = Storage::new(Arc::new(Fake::default()));
        storage
            .with_store("s3://lake/out/", Arc::new(Unabortable(InMemory::new())))
            .expect("store");
        let err = storage
            .put(
                "s3://lake/out/big.parquet",
                Bytes::from(vec![0; PART_BYTES + 1]),
            )
            .await
            .expect_err("refused");
        assert!(matches!(err, StorageError::Orphaned { .. }), "{err}");
        let failure = fossil_graph_schema::Failure::from(err);
        assert_eq!(
            failure.problem,
            fossil_graph_schema::Problem::Unreachable {
                locator: "s3://lake/out/big.parquet".into()
            }
        );
        assert!(matches!(
            failure
                .cause
                .as_ref()
                .and_then(|c| std::error::Error::source(c.as_ref()))
                .and_then(|c| c.downcast_ref::<object_store::Error>()),
            Some(object_store::Error::Generic { store: "test", .. })
        ));
        assert!(failure.help.is_some(), "the orphaned upload is said");
    }

    #[test]
    fn a_scope_is_the_hosts_wire_form() {
        let scope: Scope = serde_json::from_str(r#"{"job":"j-1"}"#).expect("json");
        assert_eq!(scope, Scope::Job("j-1".into()));
        let scope: Scope = serde_json::from_str(r#"{"connection":"sales"}"#).expect("json");
        assert_eq!(scope.to_string(), "connection sales");
    }
}
