//! W1 invariant: the registry's [`SOURCE_KINDS`] table and the runtime's
//! registered [`SourceProvider`]s are kept in lockstep — no source dispatched by
//! an ad-hoc name. Every `Provider`-lowered `SOURCE_KINDS` entry MUST have a
//! runtime provider of the same `short_name`, and every runtime provider MUST
//! have a matching `Provider` entry (with the same file extensions). This is the
//! mechanical form of "sources are added in ONE place".
//!
//! The set of runtime providers mirrors what `cmd_run` registers (currently the
//! single `RdfProvider`); adding a provider there without a `SOURCE_KINDS` entry
//! (or vice-versa) fails this test.

use std::sync::Arc;

use fossil_registry::{SOURCE_KINDS, SourceLowering};
use fossil_runtime::source_provider::SourceProvider;

/// The providers the CLI wires into its `SourceProviderRegistry` (kept in sync
/// with `cmd_run`'s registrations).
fn cli_providers() -> Vec<Arc<dyn SourceProvider>> {
    vec![Arc::new(fossil_provider_rdf::RdfProvider)]
}

#[test]
fn every_provider_source_kind_has_a_runtime_provider() {
    let providers = cli_providers();
    for kind in SOURCE_KINDS {
        if kind.lowering != SourceLowering::Provider {
            continue;
        }
        let provider = providers
            .iter()
            .find(|p| p.name() == kind.short_name)
            .unwrap_or_else(|| {
                panic!(
                    "SOURCE_KINDS has provider source `{}` but no runtime SourceProvider is registered for it",
                    kind.short_name
                )
            });
        // Extensions must match the registry's declaration (order-insensitive).
        let mut reg_ext: Vec<&str> = kind.extensions.to_vec();
        let mut prov_ext: Vec<&str> = provider.extensions().to_vec();
        reg_ext.sort_unstable();
        prov_ext.sort_unstable();
        assert_eq!(
            reg_ext, prov_ext,
            "extensions for provider `{}` disagree: SOURCE_KINDS={reg_ext:?} vs provider={prov_ext:?}",
            kind.short_name
        );
    }
}

#[test]
fn every_runtime_provider_has_a_provider_source_kind() {
    for provider in cli_providers() {
        let kind = fossil_registry::source_kind(&format!("io.{}", provider.name()))
            .unwrap_or_else(|| {
                panic!(
                    "runtime provider `{}` has no SOURCE_KINDS entry (add `io.{}` to the table)",
                    provider.name(),
                    provider.name()
                )
            });
        assert_eq!(
            kind.lowering,
            SourceLowering::Provider,
            "`io.{}` must be lowered as Provider, not a native reader",
            provider.name()
        );
    }
}
