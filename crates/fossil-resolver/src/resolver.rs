//! [`PathResolver`] trait + [`DefaultPathResolver`] standalone impl.

use crate::{ResolveError, ResolvedPath};

/// Host-provided path resolution. Implementations map the raw path string
/// the user wrote in their `.fossil` mapping to a concrete URL + cloud
/// configuration the runtime can act on.
///
/// `Send + Sync + Debug` mirrors the bound on `fossil-base::System` —
/// resolvers are typically shared across worker threads in
/// `fossil-runtime` and across request handlers in a server host.
///
/// Standalone CLIs and the playground mount [`DefaultPathResolver`].
/// Multi-tenant hosts implement this trait against their own connection
/// registry and pass an `Arc<dyn PathResolver>` through to the runtime.
pub trait PathResolver: Send + Sync + std::fmt::Debug {
    /// Resolve `raw_path` — a path string as it appeared in the user's
    /// `.fossil` source — to a concrete [`ResolvedPath`].
    ///
    /// # Errors
    ///
    /// Returns [`ResolveError::HostReferenceUnsupported`] when the path is
    /// a host-mediated reference (`@…`) and no host-side resolver is
    /// available, or [`ResolveError::Host`] for any host-side failure
    /// (credential fetch failed, connection name unknown, etc.).
    fn resolve(&self, raw_path: &str) -> Result<ResolvedPath, ResolveError>;
}

/// The standalone resolver — local + public URL paths pass through
/// unchanged; host references (`@…`) are rejected because there is no
/// host-side connection registry to resolve them against.
///
/// Mounted by `fossil-cli` and the playground.
#[derive(Debug, Default)]
pub struct DefaultPathResolver;

impl PathResolver for DefaultPathResolver {
    fn resolve(&self, raw_path: &str) -> Result<ResolvedPath, ResolveError> {
        if raw_path.starts_with('@') {
            return Err(ResolveError::HostReferenceUnsupported(raw_path.to_string()));
        }
        Ok(ResolvedPath::new(raw_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_passes_local_and_public_through() {
        let r = DefaultPathResolver;
        assert_eq!(
            r.resolve("file:///tmp/x.csv").unwrap().url(),
            "file:///tmp/x.csv"
        );
        assert_eq!(
            r.resolve("https://example.com/x.parquet").unwrap().url(),
            "https://example.com/x.parquet"
        );
        assert_eq!(
            r.resolve("s3://bucket/x.csv").unwrap().url(),
            "s3://bucket/x.csv"
        );
    }

    #[test]
    fn default_rejects_host_references() {
        let r = DefaultPathResolver;
        match r.resolve("@conn-name/file.csv") {
            Err(ResolveError::HostReferenceUnsupported(p)) => {
                assert_eq!(p, "@conn-name/file.csv");
            }
            other => panic!("expected HostReferenceUnsupported, got {other:?}"),
        }
    }
}
