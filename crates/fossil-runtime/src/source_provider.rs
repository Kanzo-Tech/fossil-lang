//! Source-provider seam — the extension point for source formats `DuckDB`
//! cannot read natively (RDF, …).
//!
//! **The language core stays format-agnostic.** `io.csv`/`io.json`/`io.parquet`
//! lower to native `DuckDB` readers (`read_csv_auto`, …) — `DuckDB` does the
//! work, no custom code. Anything `DuckDB` can't read is a *provider*: the core
//! only knows a source is "provider-backed" (`fossil_mir::SourceFormat::Provider
//! { name }`) and scans the relation the provider registers. It never sees the
//! format's decode details — those live in a crate OUTSIDE the core (e.g.
//! `fossil-provider-rdf`, which uses `srdf`).
//!
//! Mechanism, mirroring the `CREATE VIEW` prelude codegen already emits: before
//! executing a plan's source prelude, the runtime asks each provider to
//! [`materialize`](SourceProvider::materialize) its rows into the `DuckDB`
//! connection as a named relation; the prelude's `CREATE VIEW … SELECT * FROM
//! <relation>` then scans it like any other table. The provider owns the decode
//! + any shape-directed pivot; the core owns only the scan.

use std::collections::HashMap;
use std::sync::Arc;

use duckdb::Connection;

/// A pluggable reader for a source format the core does not handle natively.
///
/// Implemented OUTSIDE the language core (the impl crate owns the format's
/// decode). Registered into a [`SourceProviderRegistry`] by the host/CLI; the
/// runtime dispatches to it by [`name`](Self::name).
pub trait SourceProvider: Send + Sync {
    /// Provider name — matches the `io.<name>` source constructor and the MIR
    /// `SourceFormat::Provider { name }` the core lowered the source to.
    fn name(&self) -> &'static str;

    /// File extensions this provider reads (no leading dot). Surfaced by the
    /// host's provider listing (e.g. keasy's `/v1/providers`).
    fn extensions(&self) -> &[&str];

    /// Materialise the source at `uri` into `conn` as a relation named
    /// `relation` — a table the core's `CREATE VIEW` prelude then scans. The
    /// provider does all format-specific work here (decode + any shape-directed
    /// pivot), producing rows that match the source's declared column schema.
    ///
    /// `schema_arg` is the source constructor's `schema:` argument (e.g. a
    /// `ShEx` path), if present.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message if the source cannot be decoded or the
    /// relation cannot be created.
    fn materialize(
        &self,
        uri: &str,
        schema_arg: Option<&str>,
        relation: &str,
        conn: &Connection,
    ) -> Result<(), String>;
}

/// Registry of [`SourceProvider`]s, keyed by name.
///
/// The host (CLI) populates it; the runtime consults it before executing a
/// plan's source prelude. Empty by default — a deployment with no providers
/// behaves exactly as today (native formats only).
#[derive(Default, Clone)]
pub struct SourceProviderRegistry {
    providers: HashMap<String, Arc<dyn SourceProvider>>,
}

impl std::fmt::Debug for SourceProviderRegistry {
    // `dyn SourceProvider` is not `Debug`; show the registered names instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceProviderRegistry")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SourceProviderRegistry {
    /// An empty registry (native formats only).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider, keyed by its [`SourceProvider::name`]. A later
    /// registration with the same name replaces the earlier one.
    pub fn register(&mut self, provider: Arc<dyn SourceProvider>) {
        self.providers
            .insert(provider.name().to_string(), provider);
    }

    /// Look up a provider by name (the `SourceFormat::Provider { name }` value).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn SourceProvider>> {
        self.providers.get(name)
    }

    /// `true` if no providers are registered (the common native-only case).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that fabricates two rows — stands in for any real decoder
    /// (RDF, …) without pulling a format dependency into this RDF-free seam.
    struct DummyProvider;

    impl SourceProvider for DummyProvider {
        fn name(&self) -> &'static str {
            "dummy"
        }

        fn extensions(&self) -> &[&str] {
            &["dummy"]
        }

        fn materialize(
            &self,
            _uri: &str,
            _schema_arg: Option<&str>,
            relation: &str,
            conn: &Connection,
        ) -> Result<(), String> {
            conn.execute_batch(&format!(
                "CREATE TABLE \"{relation}\" AS \
                 SELECT * FROM (VALUES (1, 'alice'), (2, 'bob')) t(id, name);"
            ))
            .map_err(|e| e.to_string())
        }
    }

    #[test]
    fn registry_registers_and_looks_up_by_name() {
        let mut reg = SourceProviderRegistry::new();
        assert!(reg.is_empty());
        reg.register(Arc::new(DummyProvider));
        assert!(!reg.is_empty());
        assert_eq!(reg.get("dummy").map(|p| p.name()), Some("dummy"));
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn provider_materialises_a_relation_the_core_can_scan() {
        let reg = {
            let mut r = SourceProviderRegistry::new();
            r.register(Arc::new(DummyProvider));
            r
        };
        let conn = Connection::open_in_memory().expect("open duckdb");

        // The runtime would do this before executing the CREATE VIEW prelude.
        let provider = reg.get("dummy").expect("registered");
        provider
            .materialize("ignored://x.dummy", None, "__fossil_src_people", &conn)
            .expect("materialise");

        // The core's prelude (`CREATE VIEW people AS SELECT * FROM
        // __fossil_src_people`) then scans the relation like any native source.
        conn.execute_batch(
            "CREATE VIEW people AS SELECT * FROM \"__fossil_src_people\";",
        )
        .expect("prelude view");

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM people", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 2, "provider rows are visible through the core's view");

        let alice: String = conn
            .query_row("SELECT name FROM people WHERE id = 1", [], |r| r.get(0))
            .expect("row");
        assert_eq!(alice, "alice");
    }
}
