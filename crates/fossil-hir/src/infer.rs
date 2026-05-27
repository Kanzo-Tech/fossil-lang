//! Source row resolution: `MappingLoc` → `Option<Ty<'db>>` (a `Record` built
//! from an inferred or CSVW descriptor).
//!
//! # Architectural constraint (Serious #6)
//!
//! This module reads the source's `schema = "<path>"` argument from the
//! [`crate::def_map::DefMap`] (signatures-only per ADR-0005), NOT from the
//! FILE CST via a `mapping_cst_node`-walk-upward. The latter would reintroduce
//! a `parse(db, file)` dependency inside the `typecheck_mapping` query,
//! breaking the LOAD-BEARING `MAX_PER_MAPPING_FAN_OUT = 1` invariant.
//!
//! `def_map(db, file)` is file-keyed and structurally stable across body-only
//! edits: editing one mapping's body produces a structurally-equal `DefMap`
//! (same prefixes / sources / mapping-count), so Salsa's `maybe_update`
//! returns `false` and no downstream re-execution propagates. The invalidation
//! regression test (`tests/invalidation_regression.rs`) verifies this — the
//! per-mapping fan-out for `typecheck_mapping` stays at exactly 1.
//!
//! # Forward propagation (Phase 13 v0.2, ADR-0037)
//!
//! Priority order:
//!
//! 1. **`InferredDescriptor`** (preferred). When the host has pre-registered
//!    a descriptor for the mapping's source binding name via
//!    `db.system().register_inferred_descriptor(...)` (browser-side
//!    `DuckDB-WASM` via `FossilPlayground::register_inferred_descriptor`;
//!    native CLI via the `duckdb` crate), `resolve_source_row` consumes that
//!    descriptor and builds the [`Record`] directly — no CSVW JSON is read
//!    from disk.
//!
//! 2. **CSVW descriptor** (deprecated; ADR-0007 retained as intermediate IR).
//!    When the source binding declares `schema = "<path>"` AND no
//!    `InferredDescriptor` is registered, the legacy path runs: read the
//!    bytes via `db.system().read_file(...)`, parse via
//!    [`fossil_descriptors_input::CsvwDescriptor`], build the Record. The
//!    checker ALSO emits the `D-CSVW-DEPRECATED` diagnostic so v0.1 `.fossil`
//!    files see the deprecation message during their next compile.
//!
//! 3. **No descriptor**. Returns `None`; forward propagation is disabled for
//!    the mapping — `.field` accesses synthesise no type (preserves Phase 2
//!    behaviour).
//!
//! ## Salsa-safety of the inferred path
//!
//! `db.system().inferred_descriptor(source_name)` reads through the existing
//! `System` abstraction (ADR-0003 / ADR-0020). The descriptor table is NOT a
//! `salsa::input` — it is host-owned state on the System impl, mirroring
//! `read_file`. Reads from inside a tracked query do not register a Salsa
//! input dependency, so re-registering a descriptor does NOT trigger
//! invalidation. (When the host wants to invalidate, it bumps the source
//! file's text via `set_text`, which Salsa already tracks.)

use std::path::PathBuf;

use fossil_base::{Span, delay_span_bug};
use fossil_descriptors_input::{CsvwDescriptor, InferredDescriptor};
use smol_str::SmolStr;

use crate::def_map::{MappingLoc, def_map};
use crate::ty::{Primitive, Record, RecordField, Ty, TyKind};

/// Resolve the source-row [`Ty`] (a `Record`) for a mapping, if its source
/// binding declared a CSVW `schema` argument.
///
/// Plain-Rust helper (NOT `#[salsa::tracked]`) — called from within the
/// `typecheck_mapping` tracked query so its `delay_span_bug` emits are valid.
///
/// Reads `def_map(db, file)` only (NEVER walks the FILE CST from
/// `mapping_cst_node`) — see the module docs for the Serious #6 rationale.
#[must_use]
pub fn resolve_source_row<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<Ty<'db>> {
    let file = mapping.file(db);
    let dm = def_map(db, file);

    // 1. Find the mapping's source binding name.
    let mappings = crate::lower::lower_to_hir(db, file);
    let hir_mapping = mappings.mappings(db).get(mapping.index(db))?;
    let source_name = hir_mapping.source_binding.clone();

    // Phase 13 v0.2 (ADR-0037): try the host-registered `InferredDescriptor`
    // FIRST. This is the new authoring style — the user writes `io.csv("...")`
    // and the host (browser-side `DuckDB-WASM`; native CLI `duckdb` crate)
    // pre-registers the descriptor before invoking `compile`.
    if let Some(inferred) = db.system().inferred_descriptor(source_name.as_str()) {
        // If an explicit `schema = "..."` arg is ALSO present, the
        // InferredDescriptor wins (it represents fresher truth from the
        // file itself) but we ALSO emit the `D-CSVW-DEPRECATED` warning so
        // the user knows the explicit arg is now redundant.
        if dm.lookup_source_schema(db, source_name.as_str()).is_some() {
            emit_csvw_deprecated_diagnostic(db, &source_name);
        }
        return Some(record_from_inferred(db, &inferred, source_name.as_str()));
    }

    // Phase 13 v0.2 FALLBACK PATH — legacy CSVW (deprecated; still functional).

    // 2. Read the `schema = "<path>"` NAMED arg from the DefMap (signatures-
    //    only — does not re-trigger body()).
    let schema_path = dm.lookup_source_schema(db, source_name.as_str())?;

    // Explicit CSVW path is now deprecated — warn the user that v0.2 prefers
    // the inferred-descriptor path. Compilation still proceeds via CSVW.
    emit_csvw_deprecated_diagnostic(db, &source_name);

    // 3. Resolve the schema path relative to the mapping's file.
    let resolved = resolve_relative(db, file, schema_path.as_str());

    // 4. Read the bytes via the host filesystem.
    let bytes = match db.system().read_file(&resolved) {
        Ok(b) => b,
        Err(e) => {
            let span = Span::new(0, 0);
            let _eg = delay_span_bug(
                db,
                span,
                format!("cannot read CSVW schema `{schema_path}` for source `{source_name}`: {e}"),
            );
            return None;
        }
    };

    // 5. Parse via CsvwDescriptor.
    let descriptor = match CsvwDescriptor::parse(&bytes) {
        Ok(d) => d,
        Err(e) => {
            let span = Span::new(0, 0);
            let _eg = delay_span_bug(
                db,
                span,
                format!("CSVW schema `{schema_path}` failed to parse: {e}"),
            );
            return None;
        }
    };

    Some(record_from_descriptor(
        db,
        &descriptor,
        source_name.as_str(),
    ))
}

/// Build a `Record` [`Ty`] from a parsed [`CsvwDescriptor`].
///
/// Exposed (crate-public) so plan 03-05's integration tests can build a
/// source row from a hand-crafted descriptor without touching the filesystem.
#[must_use]
pub(crate) fn record_from_descriptor<'db>(
    db: &'db dyn fossil_base::Db,
    descriptor: &CsvwDescriptor,
    source_name: &str,
) -> Ty<'db> {
    let mut fields: Vec<RecordField<'db>> = Vec::new();
    for col in descriptor.columns() {
        let prim = descriptor.type_for_column(&col.name).map_or_else(
            || {
                // Column declared a datatype outside the v0.1 catalog (or no
                // datatype at all). Type it as String and emit a diagnostic
                // so the user sees WHY (P-CRIT-4: no silent coercion without
                // a diagnostic). Continue building the rest of the row.
                let _eg = delay_span_bug(
                    db,
                    Span::new(0, 0),
                    format!(
                        "CSVW column `{}` in source `{source_name}` has an \
                             unknown or missing datatype; defaulting to String",
                        col.name
                    ),
                );
                Primitive::String
            },
            primitive_from_name,
        );
        let field_ty = Ty::new(db, TyKind::Primitive(prim));
        fields.push(RecordField {
            name: SmolStr::from(col.name.as_str()),
            ty: field_ty,
        });
    }
    let rec = Record::new(db, fields);
    Ty::new(db, TyKind::Record(rec))
}

/// Resolve `schema_path` relative to the directory containing `file`'s path.
fn resolve_relative(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    schema_path: &str,
) -> PathBuf {
    let file_path = PathBuf::from(file.path(db));
    file_path
        .parent()
        .map_or_else(|| PathBuf::from(schema_path), |dir| dir.join(schema_path))
}

/// Inverse of `CsvwDescriptor::type_for_column`'s `&'static str` name table.
///
/// Lives here (in `fossil-hir`) per plan 03-01 / 03-02's cycle-avoidance
/// design: `fossil-descriptors-input` returns the canonical [`Primitive`]
/// variant *name* as a string to avoid importing `fossil-hir::ty::Primitive`;
/// the string → enum conversion happens here at the consuming boundary.
#[must_use]
fn primitive_from_name(name: &str) -> Primitive {
    match name {
        "Integer" => Primitive::Integer,
        "Float" => Primitive::Float,
        "Bool" => Primitive::Bool,
        "Date" => Primitive::Date,
        "DateTime" => Primitive::DateTime,
        "Time" => Primitive::Time,
        "GYear" => Primitive::GYear,
        "AnyURI" => Primitive::AnyURI,
        // "String" and any unexpected name fall back to String.
        _ => Primitive::String,
    }
}

/// Returns `true` iff `name` is one of the canonical [`Primitive`] variant
/// names recognised by [`primitive_from_name`]. Used by
/// [`record_from_inferred`] to emit a diagnostic when a host-provided
/// `InferredColumn.primitive` string falls outside the catalog (mirrors the
/// CSVW path's "unknown datatype → String" behaviour in
/// [`record_from_descriptor`]).
#[must_use]
fn is_canonical_primitive_name(name: &str) -> bool {
    matches!(
        name,
        "Integer"
            | "Float"
            | "String"
            | "Bool"
            | "Date"
            | "DateTime"
            | "Time"
            | "GYear"
            | "AnyURI"
    )
}

/// Build a `Record` [`Ty`] from a host-provided [`InferredDescriptor`].
///
/// Phase 13 v0.2 (ADR-0037) inferred-path companion to
/// [`record_from_descriptor`]. The two functions produce structurally
/// equivalent Records on identical column shapes (semantic-equivalence
/// invariant from INPUT-03). When a column carries a non-canonical
/// `primitive` string, this function defaults to `String` and emits a
/// `D-INFERRED-UNKNOWN-DATATYPE` diagnostic (mirrors the CSVW path's
/// behaviour on unknown CSVW datatypes — see [`record_from_descriptor`]).
#[must_use]
pub(crate) fn record_from_inferred<'db>(
    db: &'db dyn fossil_base::Db,
    inferred: &InferredDescriptor,
    source_name: &str,
) -> Ty<'db> {
    let mut fields: Vec<RecordField<'db>> = Vec::new();
    for col in &inferred.columns {
        if !is_canonical_primitive_name(col.primitive.as_str()) {
            let _eg = delay_span_bug(
                db,
                Span::new(0, 0),
                format!(
                    "D-INFERRED-UNKNOWN-DATATYPE: inferred column `{}` in source \
                     `{source_name}` has non-canonical primitive `{}`; defaulting \
                     to String",
                    col.name, col.primitive
                ),
            );
        }
        let prim = primitive_from_name(col.primitive.as_str());
        let field_ty = Ty::new(db, TyKind::Primitive(prim));
        fields.push(RecordField {
            name: col.name.clone(),
            ty: field_ty,
        });
    }
    let rec = Record::new(db, fields);
    Ty::new(db, TyKind::Record(rec))
}

/// Emit the `D-CSVW-DEPRECATED` warning when a source binding's explicit
/// `schema = "..."` arg is encountered (ADR-0037).
///
/// Severity is conveyed via the `D-CSVW-DEPRECATED:` text prefix consumed
/// downstream by the diagnostic renderer; the underlying `delay_span_bug`
/// accumulator is the existing Phase-3 channel (a dedicated `warning`
/// accumulator is out-of-scope for plan 13-02).
fn emit_csvw_deprecated_diagnostic(db: &dyn fossil_base::Db, source_name: &SmolStr) {
    let _eg = delay_span_bug(
        db,
        Span::new(0, 0),
        format!(
            "D-CSVW-DEPRECATED: explicit CSVW descriptor for source \
             `{source_name}` is deprecated; types will be inferred from the \
             file directly. Remove the `schema = \"...\"` argument."
        ),
    );
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    const USERS_CSVW: &str = r#"{
      "@context": "http://www.w3.org/ns/csvw",
      "url": "users.csv",
      "tableSchema": {
        "columns": [
          { "name": "id", "datatype": "integer" },
          { "name": "name", "datatype": "string" },
          { "name": "age", "datatype": "integer" }
        ]
      }
    }"#;

    #[test]
    fn record_from_descriptor_maps_columns_to_primitives() {
        let db = db();
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).expect("valid CSVW");
        let row = record_from_descriptor(&db, &descriptor, "users");
        let TyKind::Record(rec) = row.kind(&db) else {
            panic!("expected Record, got {:?}", row.kind(&db));
        };
        let fields = rec.fields(&db);
        assert_eq!(fields.len(), 3);
        let id = fields.iter().find(|f| f.name == "id").expect("id field");
        assert_eq!(id.ty.kind(&db), &TyKind::Primitive(Primitive::Integer));
        let name = fields
            .iter()
            .find(|f| f.name == "name")
            .expect("name field");
        assert_eq!(name.ty.kind(&db), &TyKind::Primitive(Primitive::String));
        let age = fields.iter().find(|f| f.name == "age").expect("age field");
        assert_eq!(age.ty.kind(&db), &TyKind::Primitive(Primitive::Integer));
    }

    #[test]
    fn primitive_from_name_round_trips_all_variants() {
        assert_eq!(primitive_from_name("Integer"), Primitive::Integer);
        assert_eq!(primitive_from_name("Float"), Primitive::Float);
        assert_eq!(primitive_from_name("String"), Primitive::String);
        assert_eq!(primitive_from_name("Bool"), Primitive::Bool);
        assert_eq!(primitive_from_name("Date"), Primitive::Date);
        assert_eq!(primitive_from_name("DateTime"), Primitive::DateTime);
        assert_eq!(primitive_from_name("Time"), Primitive::Time);
        assert_eq!(primitive_from_name("GYear"), Primitive::GYear);
        assert_eq!(primitive_from_name("AnyURI"), Primitive::AnyURI);
    }

    // Phase 13 v0.2 (ADR-0037, INPUT-03 semantic equivalence)
    #[test]
    fn record_from_inferred_matches_csvw_path_on_same_shape() {
        use fossil_descriptors_input::{InferredColumn, InferredDescriptor};

        let db = db();

        // Build via the legacy CSVW path.
        let csvw_descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).expect("valid CSVW");
        let row_csvw = record_from_descriptor(&db, &csvw_descriptor, "users");

        // Build the equivalent InferredDescriptor (same column shape).
        let inferred = InferredDescriptor {
            source_name: "users".into(),
            columns: vec![
                InferredColumn {
                    name: "id".into(),
                    primitive: "Integer".into(),
                },
                InferredColumn {
                    name: "name".into(),
                    primitive: "String".into(),
                },
                InferredColumn {
                    name: "age".into(),
                    primitive: "Integer".into(),
                },
            ],
            content_hash: "test-hash".into(),
        };
        let row_inferred = record_from_inferred(&db, &inferred, "users");

        // Both paths must produce a Record with structurally-identical fields.
        let TyKind::Record(rec_csvw) = row_csvw.kind(&db) else {
            panic!("CSVW: expected Record");
        };
        let TyKind::Record(rec_inferred) = row_inferred.kind(&db) else {
            panic!("Inferred: expected Record");
        };
        let f_csvw = rec_csvw.fields(&db);
        let f_inferred = rec_inferred.fields(&db);
        assert_eq!(f_csvw.len(), f_inferred.len());
        for (a, b) in f_csvw.iter().zip(f_inferred.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.ty.kind(&db), b.ty.kind(&db));
        }
    }

    // Phase 13 v0.2 — guard the canonical-primitive table used by
    // `record_from_inferred` for D-INFERRED-UNKNOWN-DATATYPE emission.
    #[test]
    fn is_canonical_primitive_name_matches_primitive_from_name_table() {
        for name in [
            "Integer", "Float", "String", "Bool", "Date", "DateTime", "Time", "GYear", "AnyURI",
        ] {
            assert!(is_canonical_primitive_name(name), "`{name}` is canonical");
        }
        assert!(!is_canonical_primitive_name("integer")); // case-sensitive
        assert!(!is_canonical_primitive_name("Decimal")); // outside catalog
        assert!(!is_canonical_primitive_name(""));
    }

    // Phase 13 v0.2 — `primitive_from_name` already covers the unknown-primitive
    // → String fallback (see `primitive_from_name_round_trips_all_variants` for
    // the canonical names; the wildcard `_ => Primitive::String` arm handles
    // unknowns). A full `record_from_inferred` test for the unknown branch
    // requires running inside a tracked Salsa query (the `D-INFERRED-UNKNOWN-
    // DATATYPE` `delay_span_bug` only accumulates inside `#[salsa::tracked]`
    // — outside one, the accumulator panics). The downstream e2e path
    // exercised by `record_from_inferred_matches_csvw_path_on_same_shape`
    // covers the canonical fast-path.
}
