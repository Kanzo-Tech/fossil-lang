//! Typed errors for path resolution.
//!
//! Distinguishes the standalone-mode failure (host references unavailable)
//! from arbitrary host-supplied failures so callers can pattern-match
//! instead of substring-matching on a free-form message.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolveError {
    /// The path is a host-mediated reference (e.g. `@conn-name/users.csv`)
    /// and no host-side resolver is mounted.
    ///
    /// Returned by [`crate::DefaultPathResolver`] as its only failure case.
    /// Multi-tenant hosts (Keasy) replace the default resolver with one
    /// that knows how to map `@conn-name` to a concrete URL + per-org
    /// credentials and therefore never produce this variant.
    #[error("host reference (`{0}`) not available — no host-side resolver mounted")]
    HostReferenceUnsupported(String),

    /// Host-provided resolvers surface arbitrary failures here (credential
    /// fetch failed, connection name unknown, signed URL minting refused
    /// by the upstream provider, …).
    ///
    /// The `String` payload is the host's diagnostic message — bindings
    /// (fossil-cli, fossil-mcp, keasy proxy) surface it verbatim to the
    /// caller. Hosts that want a typed error of their own wrap their error
    /// in their crate and convert it to a string at this boundary; the
    /// resolver crate stays free of host-specific error enums.
    #[error("resolver error: {0}")]
    Host(String),
}

impl ResolveError {
    /// Wrap a host-side error message as [`ResolveError::Host`]. Helper for
    /// resolvers that hold their own typed error and want to forward its
    /// `Display` rendering without introducing a `From` impl per host.
    pub fn host(msg: impl Into<String>) -> Self {
        Self::Host(msg.into())
    }
}
