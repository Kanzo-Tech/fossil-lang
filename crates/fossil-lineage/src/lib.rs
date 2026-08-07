//! Source lineage + provider introspection — the parse-only "what does this
//! program reference?" and "what sources does fossil support?" surface.
//!
//! Both hosts consume one implementation (ADR-0024 "one crate, two hosts"): the
//! native `fossil-engine`/`fossil-cli` (via [`source_refs`] over a file-backed
//! db) and the browser `fossil-wasm` (over its in-memory `WasmDb`). The host
//! injects only its own [`fossil_base::Db`] + program text; the logic — parse →
//! source headers → typed refs — is identical and pure (no I/O, no `DuckDB`),
//! so it is WASM-clean.
//!
//! [`providers`] is a projection of [`fossil_hir::stdlib::SOURCE_KINDS`] and
//! [`source_refs`] walks the def map, so its content is the language's. What
//! keeps it out of `fossil-hir` is the other end: the result types are
//! `fossil-run-status`, the host wire contract, and ADR-0046 F8 dissolves that
//! into the shell. This crate is the projection onto that wire, so it moves
//! when the wire does. Until ADR-0045 §4 the projection sat in `fossil-ide`
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
/// A destructuring `{ A, B } := io.rdf(uri, schema = "x")` expands to one
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

/// The data-source providers fossil supports, projected from
/// [`fossil_hir::stdlib::SOURCE_KINDS`] (the single source of truth — native
/// readers AND external providers like `rdf`). Sorted for a deterministic
/// order (the registry's own iteration order is unspecified).
#[must_use]
pub fn providers() -> Vec<ProviderInfo> {
    let mut providers: Vec<ProviderInfo> = fossil_hir::stdlib::SOURCE_KINDS
        .iter()
        .map(|k| ProviderInfo {
            name: k.short_name.to_string(),
            extensions: k.extensions.iter().map(|e| (*e).to_string()).collect(),
            kind: ProviderKind::Data,
        })
        .collect();
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    providers
}

#[cfg(test)]
mod tests {
    use super::providers;

    #[test]
    fn providers_are_sorted_nonempty_and_include_csv() {
        let p = providers();
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
    }
}
