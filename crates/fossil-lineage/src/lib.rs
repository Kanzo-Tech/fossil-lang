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
//! keeps it out of `fossil-hir` is the other end: the result types are
//! `fossil-run-status`, the host wire contract, and that contract dissolves
//! into the shell. This crate is the projection onto that wire, so it moves
//! when the wire does. Until this crate existed the projection sat in `fossil-ide`
//! while the data sat in the registry and the result type in
//! `fossil-run-status` — one idea across three crates that did not know each
//! other, and it made `fossil-engine` depend on the editor surface for five
//! lines.

use fossil_base::{Db, SourceFile};
use fossil_run_status::{ProviderInfo, ProviderKind, RefRole, SourceRefInfo};

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
    use fossil_run_status::ProviderKind;

    use super::providers;

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
