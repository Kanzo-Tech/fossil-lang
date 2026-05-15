//! `GraphAr` manifest emission — Phase 1 hand-templated constant.
//!
//! Phase 5 (SINK-02) replaces this with programmatic generation via
//! `serde_yaml_ng` + a `Manifest` struct (the workspace pin already exists;
//! adding the dep is a Phase 5 ergonomic concern, not a Phase 1 blocker).
//!
//! Phase 5 (SINK-01) brings `ShEx`-driven vertex/edge decomposition in line.
//! Until then, the manifest claims a single `Person` vertex with a `name`
//! property — aspirational, since Phase 1 emits a flat triple Parquet that
//! does not yet conform to the `GraphAr` per-vertex chunk layout.

/// Return the Phase 1 hand-templated `GraphAr` manifest. Phase 5 SINK-02
/// promotes this to programmatic generation.
#[must_use]
pub fn manifest_template() -> String {
    // Trailing newline is intentional — keeps the file POSIX-clean and
    // matches the snapshot.
    "\
# GraphAr manifest — Phase 1 skeletal form.
graphar_version: 1.0.0
prefix: https://example.org/
vertex_types:
  - name: Person
    chunk_size: 1024
    properties:
      - name: name
        data_type: string
        nullable: false
    parquet_path: output.parquet
edge_types: []
"
    .to_string()
}
