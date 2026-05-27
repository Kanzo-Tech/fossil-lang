//! Source row resolution: `MappingLoc` → `Option<Ty<'db>>` (a `Record` built
//! from a CSVW descriptor).
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
//! # Forward propagation
//!
//! When a mapping's `from` source declares `schema = "<path>"`, the path is
//! resolved relative to the mapping's file, the bytes are read via
//! `db.system().read_file(...)`, parsed by
//! [`fossil_descriptors_input::CsvwDescriptor`], and each CSVW column becomes
//! a [`crate::ty::RecordField`]. `.field` accesses in the mapping body then
//! resolve against this `Record` (plan 03-05's `Checker::lookup_field`).
//!
//! When NO `schema` arg is present (the walking-skeleton `hello.fossil` case),
//! this returns `None` and forward propagation is disabled for the mapping —
//! `.field` accesses synthesise no type (preserving the Phase 2 behaviour).

use std::path::PathBuf;

use fossil_base::{Span, delay_span_bug};
use fossil_descriptors_input::CsvwDescriptor;
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

    // 2. Read the `schema = "<path>"` NAMED arg from the DefMap (signatures-
    //    only — does not re-trigger body()).
    let schema_path = dm.lookup_source_schema(db, source_name.as_str())?;

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
}
