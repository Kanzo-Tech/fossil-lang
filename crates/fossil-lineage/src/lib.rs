//! Source lineage + provider introspection — the parse-only "what does this
//! program reference?" and "what sources does fossil support?" surface.
//!
//! Both hosts consume one implementation — one crate, two hosts: the
//! native `fossil-engine`/`fossil-cli` (via [`source_refs`] over a file-backed
//! db) and the browser `fossil-wasm` (over its in-memory `WasmDb`). The host
//! injects only its own [`fossil_base::Db`] + program text; the logic — parse →
//! source headers → typed refs — is identical and pure (no I/O, no `DuckDB`),
//! so it is WASM-clean.
//!
//! [`providers`] is a projection of [`fossil_base::providers`] and
//! [`source_refs`] walks the def map, so its content is the language's. What
//! keeps it out of `fossil-hir` is the other end: these are the shapes a HOST
//! reads — serde JSON on the CLI's stdout, `serde-wasm-bindgen` values in the
//! browser — and `fossil-hir` answers to the compiler, not to a host.
//!
//! The four types below used to live in `fossil-run-status`, which this crate
//! imported to name its own output: the projection sat here, the data in the
//! registry, and the result type in a third crate that nothing but this one
//! produced. A contract belongs to whoever fills it.

use fossil_base::{Db, SourceFile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The position a reference plays in an `io.*` source constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefRole {
    /// The positional data URI (`io.rdf("…")`).
    Data,
    /// The `schema = io.shex("…")` argument (a shape document).
    Schema,
}

/// One external reference a program makes. `connection` is the `@conn` alias the
/// reference targets (`Some("cpi")` for `@cpi/graph.ttl`), or `None` for a direct
/// URL / local path. `path` is the remainder after the alias (or the whole
/// locator when there is no alias). This is the program's TYPED lineage — a host
/// derives a job's connection set from the distinct `connection`s, never from a
/// regex over the script text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRefInfo {
    /// The `@conn` alias this reference targets, or `None` for a direct URL/path.
    pub connection: Option<String>,
    /// The path within the connection, or the whole locator when unaliased.
    pub path: String,
    /// Where this reference appears in the source constructor.
    pub role: RefRole,
}

/// Parse-only typed lineage: every external reference a program makes — its
/// data URIs and `schema =` arguments — each tagged with the `@conn` alias it
/// targets (or `None` for a direct URL/path). The host reads this to derive a
/// job's connection set without scanning script text.
///
/// A destructuring `{ A, B } := io.rdf(uri, schema = io.shex("x.shex"))` expands to one
/// [`fossil_hir::def_map::SourceEntry`] per member sharing the same uri +
/// schema, so identical refs are de-duplicated — a job's lineage is the
/// DISTINCT `(data, schema)` it reads.
#[must_use]
pub fn source_refs(db: &dyn Db, file: SourceFile) -> Vec<SourceRefInfo> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mut refs: Vec<SourceRefInfo> = Vec::new();
    for s in def_map.sources(db) {
        if let Some(uri) = s.uri.as_deref() {
            let r = parse_ref(uri, RefRole::Data);
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
        if let Some(schema) = s.schema_arg.as_deref() {
            let r = parse_ref(schema, RefRole::Schema);
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    }
    refs
}

/// Split a raw reference into its `@conn` alias + path, or `None` + the whole
/// locator. Reports the ALIAS, not the resolved URL — resolution is the host's
/// data-plane job (`@conn` → `{base}/path`), kept out of fossil's semantics.
fn parse_ref(raw: &str, role: RefRole) -> SourceRefInfo {
    match raw.strip_prefix('@').and_then(|r| r.split_once('/')) {
        Some((conn, path)) => SourceRefInfo {
            connection: Some(conn.to_string()),
            path: path.to_string(),
            role,
        },
        None => SourceRefInfo {
            connection: None,
            path: raw.to_string(),
            role,
        },
    }
}

/// What a provider can appear as in a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Only defines a type (e.g. a schema descriptor).
    Schema,
    /// Loads data (the `io.*` source constructors).
    Data,
    /// Usable in both positions.
    Both,
}

/// One data-source provider fossil exposes: its short name, the file extensions
/// it reads, and how it can be used. A host lists these so its UI can offer the
/// constructors and filter files by extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderInfo {
    /// Short provider name (e.g. `csv`, `json`, `parquet`).
    pub name: String,
    /// File extensions this provider reads (no leading dot).
    pub extensions: Vec<String>,
    /// Whether the provider defines a type, loads data, or both.
    pub kind: ProviderKind,
}

/// The providers a host installs, projected onto the wire contract. Sorted for
/// a deterministic order (the registry's own order is priority, not display).
///
/// # `ProviderKind` stops being a constant
///
/// This function wrote `ProviderKind::Data` on every row, and the wire contract
/// has carried `Schema` and `Both` since it was written with nothing ever
/// producing either — the shape of the answer was right and the data behind it
/// was half a table. Since ruling 13 the row declares its capabilities, so the
/// three cases are read off it: `io.shex` and `io.shacl` come out `Schema`, and
/// the day one row does both, `Both`.
///
/// `table` is the host's, not a constant, because the rows that read types are
/// the host's to install — `fossil_descriptors_output::PROVIDERS` for a host
/// that compiles, `fossil_base::providers::DATA` for one that does not.
#[must_use]
pub fn providers(table: &[&'static fossil_base::Provider]) -> Vec<ProviderInfo> {
    use fossil_base::Capability;

    let mut providers: Vec<ProviderInfo> = table
        .iter()
        .map(|p| ProviderInfo {
            name: p.name.to_string(),
            extensions: p.extensions.iter().map(|e| (*e).to_string()).collect(),
            kind: match (
                p.provides(Capability::ReadRows),
                p.provides(Capability::ReadTypes),
            ) {
                (true, true) => ProviderKind::Both,
                (false, true) => ProviderKind::Schema,
                _ => ProviderKind::Data,
            },
        })
        .collect();
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    providers
}

#[cfg(test)]
mod tests {
    use super::{ProviderKind, providers};

    #[test]
    fn providers_are_sorted_nonempty_and_include_csv() {
        let p = providers(fossil_base::providers::DATA);
        assert!(!p.is_empty(), "the source registry must expose providers");
        let mut sorted = p.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            p, sorted,
            "providers must be deterministically sorted by name"
        );
        let names: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        assert!(
            names.contains(&"csv"),
            "csv provider must be present: {names:?}"
        );
        assert!(
            p.iter().all(|x| x.kind == ProviderKind::Data),
            "the default table reads data and nothing else"
        );
    }

    /// `ProviderKind::Schema` had no producer at all until the registry
    /// collapsed: the wire contract described three cases and the data behind it
    /// was half a table. A row that reads types comes out `Schema`.
    #[test]
    fn a_type_reading_row_projects_as_a_schema_provider() {
        use fossil_base::Provider;
        use fossil_base::test_support::decode_lines;

        static ROW: Provider = Provider {
            name: "shex",
            extensions: &["shex"],
            reads_rows: None,
            // Any `DecodeTypes` will do — what is under test is the projection,
            // and `fossil-base`'s reference decoder is the one row this crate can
            // name without learning a schema language.
            reads_types: Some(decode_lines),
        };

        let p = providers(&[&fossil_base::providers::CSV, &ROW]);
        assert_eq!(p[0].name, "csv");
        assert_eq!(p[0].kind, ProviderKind::Data);
        assert_eq!(p[1].name, "shex");
        assert_eq!(p[1].kind, ProviderKind::Schema);
    }
}
