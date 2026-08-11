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
//!    a descriptor for the mapping's source **URI** via
//!    `db.system().descriptors()` (browser-side `DuckDB-WASM` via
//!    `FossilPlayground::registerInferredDescriptor`; the native engine via
//!    the `duckdb` crate), `resolve_source_row` consumes that descriptor and
//!    builds the [`Record`] directly — no CSVW JSON is read from disk.
//!
//!    The lookup key is the URI as the program writes it, not the binding
//!    name (ADR-0050): the descriptor describes a file, and two bindings may
//!    name one. A source whose RHS is not a recognisable `io.*("…")` call has
//!    no URI, hence no descriptor, and falls through to 2 or 3.
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
//! `db.system().descriptors()` reads through the existing `System`
//! abstraction (ADR-0003 / ADR-0020). The descriptor table is NOT a
//! `salsa::input` — it is host-owned state on the System impl, mirroring
//! `read_file`. Reads from inside a tracked query do not register a Salsa
//! input dependency, so re-registering a descriptor does NOT trigger
//! invalidation. (When the host wants to invalidate, it bumps the source
//! file's text via `set_text`, which Salsa already tracks.)

use fossil_base::{Span, delay_span_bug};
use fossil_descriptors_input::{CsvwDescriptor, InferredDescriptor};
use salsa::Accumulator;
use smol_str::SmolStr;

use fossil_graph_schema::Primitive;

use crate::def_map::{MappingLoc, ShapeBindError, def_map, resolve_relative};
use crate::ty::display::render_ty_kind;
use crate::ty::{Record, RecordField, Ty, TyKind};

/// The source-row [`Ty`] (a `Record`) for a mapping as known from the
/// host-registered [`fossil_descriptors_input::InferredDescriptor`] ONLY.
///
/// Side-effect-free: no diagnostics, no CSVW fallback, no filesystem reads —
/// the descriptor-branch of [`resolve_source_row`] without the type-check's
/// `delay_span_bug` / `D-CSVW-DEPRECATED` emission. For IDE features
/// (completion, hover) that want the source schema OUTSIDE a tracked query.
/// Returns `None` when no descriptor is registered for the mapping's source
/// URI (e.g. the host did not pre-introspect, or the legacy CSVW path is
/// in use — those callers want [`resolve_source_row`] inside type-check).
#[must_use]
pub fn source_row_inferred<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<Ty<'db>> {
    let file = mapping.file(db);
    let mappings = crate::lower::lower_to_hir(db, file);
    let hir_mapping = mappings.mappings(db).get(mapping.index(db))?;
    let source_name = hir_mapping.source_binding.clone();
    let inferred = lookup_inferred(db, def_map(db, file), source_name.as_str())?;
    Some(record_from_inferred(db, &inferred))
}

/// The descriptor registered for the URI `source_name` is bound to, if the
/// binding has a URI at all and the host keeps a table.
///
/// The indirection binding-name → URI → descriptor is the whole of ADR-0050 on
/// the consumer side: the `DefMap` already carries the positional URI (it has
/// to, `fossil-mir::lower` reads it for `Op::Source`), so no new datum crosses
/// a query boundary and the per-mapping fan-out is unchanged.
fn lookup_inferred<'db>(
    db: &'db dyn fossil_base::Db,
    dm: crate::def_map::DefMap<'db>,
    source_name: &str,
) -> Option<InferredDescriptor> {
    let (_ctor, uri) = dm.lookup_source_call(db, source_name)?;
    db.system().descriptors()?.get(uri?.as_str())
}

/// Map one [`InferredColumn`] to a [`RecordField`] — the column→field lowering
/// shared by [`record_from_inferred`] (type-check) and [`source_row_inferred`]
/// (IDE).
fn field_from_inferred<'db>(
    db: &'db dyn fossil_base::Db,
    col: &fossil_descriptors_input::InferredColumn,
) -> RecordField<'db> {
    RecordField {
        name: col.name.clone(),
        ty: Ty::new(db, TyKind::Primitive(col.primitive)),
    }
}

/// Resolve the source-row [`Ty`] (a `Record`) for a mapping, if its source
/// binding declared a CSVW `schema` argument.
///
/// Plain-Rust helper (NOT `#[salsa::tracked]`) — called from within the
/// `typecheck_mapping` tracked query so its `delay_span_bug` emits are valid.
///
/// Reads `def_map(db, file)` only (NEVER walks the FILE CST from
/// `mapping_cst_node`) — see the module docs for the Serious #6 rationale.
pub fn resolve_source_row<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Result<Option<Ty<'db>>, fossil_base::ErrorGuaranteed> {
    let file = mapping.file(db);

    // 1. Find the mapping's source binding name.
    let mappings = crate::lower::lower_to_hir(db, file);
    let Some(hir_mapping) = mappings.mappings(db).get(mapping.index(db)) else {
        return Ok(None);
    };
    let source_name = hir_mapping.source_binding.clone();

    resolve_binding_row(db, file, source_name.as_str(), 0)
}

/// A source pipeline that derives from a pipeline that derives from … Thirty-two
/// is not a limit anyone will meet writing a mapping; it is the depth at which a
/// CYCLE (`a := b |> where(…)`, `b := a |> where(…)`) stops being an infinite
/// loop and becomes a diagnostic.
const MAX_PIPE_DEPTH: usize = 32;

/// The row type of a source BINDING, by name.
///
/// Split out of [`resolve_source_row`] because a pipeline's row is its base's
/// row transformed, and the base is a binding, not a mapping. Everything below
/// the pipeline branch is what the function has always done for a binding that
/// reads a file.
pub fn resolve_binding_row<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    source_name: &str,
    depth: usize,
) -> Result<Option<Ty<'db>>, fossil_base::ErrorGuaranteed> {
    let dm = def_map(db, file);

    // A binding whose right-hand side is a pipeline: the row is the base's row
    // put through the verbs (ADR-0054). A binding with no descriptor still has
    // no row — deriving from nothing gives nothing, and the algebra says so by
    // returning `None` rather than inventing column names.
    let hir = crate::lower::lower_to_hir(db, file);
    if let Some(pipe) = hir
        .source_pipes(db)
        .iter()
        .find(|p| p.name.as_str() == source_name)
    {
        if depth >= MAX_PIPE_DEPTH {
            return Err(pipe_error(
                db,
                pipe,
                format!(
                    "the source pipeline `{source_name}` derives from itself, directly or \
                     through the pipelines it names"
                ),
            ));
        }
        // A base with no descriptor has no row, and a pipeline over it has none
        // either: there is nothing to restrict, union or check a key against.
        // That is not an error — it is every schemaless program in the tree.
        let Some(mut row) = resolve_binding_row(db, file, pipe.base.as_str(), depth + 1)? else {
            return Ok(None);
        };
        for op in &pipe.ops {
            row = apply_source_op(db, file, pipe, op, row, depth)?;
        }
        return Ok(Some(row));
    }

    // Phase 13 v0.2 (ADR-0037): try the host-registered `InferredDescriptor`
    // FIRST. This is the new authoring style — the user writes `io.csv("...")`
    // and the host (browser-side `DuckDB-WASM`; native CLI `duckdb` crate)
    // pre-registers the descriptor before invoking `compile`.
    if let Some(inferred) = lookup_inferred(db, dm, source_name) {
        // If an explicit `schema = "..."` arg is ALSO present, the
        // InferredDescriptor wins (it represents fresher truth from the
        // file itself) but we ALSO emit the `D-CSVW-DEPRECATED` warning so
        // the user knows the explicit arg is now redundant.
        if dm.lookup_source_schema(db, source_name).is_some() {
            emit_csvw_deprecated_diagnostic(db, &SmolStr::from(source_name));
        }
        return Ok(Some(record_from_inferred(db, &inferred)));
    }

    // RDF destructuring source member (`{ A, B } := io.rdf(..., schema =
    // "x.shex")`): the member's shape, resolved at COMPILE TIME, IS the source
    // row type. One column per shape constraint (literal datatype → typed
    // Primitive, shape-ref → IRI String) plus the `subject` IRI column every
    // pivoted RDF row carries (so `iri = .subject` types). This is the same
    // "schema → Record at compile time" path CSV uses — no runtime column
    // resolution in the provider.
    if let Some(shape_iri) = dm.lookup_source_shape_iri(db, source_name) {
        let Some(schema_path) = dm.lookup_source_schema(db, source_name) else {
            return Ok(None);
        };
        let resolved = resolve_relative(db, file, schema_path.as_str());
        let bytes = match db.system().read_file(&resolved) {
            Ok(b) => b,
            Err(e) => {
                let _eg = delay_span_bug(
                    db,
                    Span::new(0, 0),
                    format!("cannot read ShEx `{schema_path}` for source `{source_name}`: {e}"),
                );
                return Ok(None);
            }
        };
        let desc = match fossil_descriptors_output::ShExDescriptor::from_reader(bytes.as_slice()) {
            Ok(d) => d,
            Err(e) => {
                let _eg = delay_span_bug(
                    db,
                    Span::new(0, 0),
                    format!("ShEx `{schema_path}` failed to parse: {e:?}"),
                );
                return Ok(None);
            }
        };
        return Ok(Some(record_from_shape(db, &desc, shape_iri.as_str())));
    }

    // A destructuring member that bound no shape, reported by CAUSE. One
    // message used to cover all four and blamed the member's name for every
    // one of them, so an unreadable file read as a misspelt name. Binding is
    // positional now (ADR-0057, tenth amendment), so "matches no shape" is not
    // among the causes any more — the name is a free local label.
    if let Some(err) = dm.lookup_source_shape_error(db, source_name) {
        let message = match err {
            ShapeBindError::NoSchema => format!(
                "destructuring source `{source_name}` has no `schema = \"…\"`, \
                 so there is no document to take shapes from"
            ),
            ShapeBindError::Unreadable { path, cause } => {
                format!("cannot read shape document `{path}` for `{source_name}`: {cause}")
            }
            ShapeBindError::Unparseable { path, cause } => {
                format!("shape document `{path}` for `{source_name}` failed to parse: {cause}")
            }
            ShapeBindError::Arity { declared, named } => format!(
                "the binding names {named} shape(s) and the document declares {declared}; \
                 names bind by position, so `{source_name}` has no shape to bind"
            ),
        };
        let _eg = delay_span_bug(db, Span::new(0, 0), message);
        return Ok(None);
    }

    // Phase 13 v0.2 FALLBACK PATH — legacy CSVW (deprecated; still functional).

    // 2. Read the `schema = "<path>"` NAMED arg from the DefMap (signatures-
    //    only — does not re-trigger body()).
    let Some(schema_path) = dm.lookup_source_schema(db, source_name) else {
        return Ok(None);
    };

    // Explicit CSVW path is now deprecated — warn the user that v0.2 prefers
    // the inferred-descriptor path. Compilation still proceeds via CSVW.
    emit_csvw_deprecated_diagnostic(db, &SmolStr::from(source_name));

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
            return Ok(None);
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
            return Ok(None);
        }
    };

    Ok(Some(record_from_descriptor(db, &descriptor, source_name)))
}

/// One verb of a source pipeline applied to the row it receives (ADR-0054 §4).
///
/// Every refusal names the pipeline and the columns it actually has: a row
/// algebra whose errors say "column not found" and stop is a row algebra nobody
/// can debug from the message.
fn apply_source_op<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    pipe: &crate::lower::HirSourcePipe,
    op: &crate::lower::HirSourceOp,
    row: Ty<'db>,
    depth: usize,
) -> Result<Ty<'db>, fossil_base::ErrorGuaranteed> {
    use crate::lower::HirSourceOp;

    let Some(fields) = record_fields(db, row) else {
        return Ok(row);
    };
    match op {
        // `where` keeps rows, not columns: the row type is its input's. The
        // columns the predicate names still have to exist — a filter on a column
        // that is not there is a program that would run and keep everything.
        HirSourceOp::Where(pred) => {
            let mut named = Vec::new();
            collect_field_refs(pred, &mut named);
            for col in named {
                if !fields.iter().any(|f| f.name == col) {
                    return Err(pipe_error(
                        db,
                        pipe,
                        format!(
                            "`where` in `{}` reads `.{col}`, which `{}` does not have. It has: {}",
                            pipe.name,
                            pipe.base,
                            column_list(&fields),
                        ),
                    ));
                }
            }
            Ok(row)
        }
        HirSourceOp::Select(cols) => {
            let mut kept = Vec::with_capacity(cols.len());
            for col in cols {
                let Some(f) = fields.iter().find(|f| &f.name == col) else {
                    return Err(pipe_error(
                        db,
                        pipe,
                        format!(
                            "`select` in `{}` names `.{col}`, which its input does not have. \
                             It has: {}",
                            pipe.name,
                            column_list(&fields),
                        ),
                    ));
                };
                kept.push(f.clone());
            }
            Ok(Ty::new(db, TyKind::Record(Record::new(db, kept))))
        }
        // The join: `fila(izq) ⊎ fila(der)` with the key identified once, and any
        // OTHER shared name an error. Both halves of that are load-bearing — the
        // key is exempt because `on = .k` is `USING (k)`, which identifies the two
        // columns rather than duplicating them.
        HirSourceOp::Join { right, key } => {
            let Some(right_row) = resolve_binding_row(db, file, right.as_str(), depth + 1)? else {
                return Err(pipe_error(
                    db,
                    pipe,
                    format!(
                        "`join` in `{}` joins `{right}`, whose columns are unknown — it declares \
                         no schema, so there is nothing to check the key against",
                        pipe.name
                    ),
                ));
            };
            let Some(right_fields) = record_fields(db, right_row) else {
                return Ok(row);
            };

            let (Some(lk), Some(rk)) = (
                fields.iter().find(|f| &f.name == key),
                right_fields.iter().find(|f| &f.name == key),
            ) else {
                return Err(pipe_error(
                    db,
                    pipe,
                    format!(
                        "`join` in `{}` is on `.{key}`, which has to exist on both sides. \
                         The left has: {}. `{right}` has: {}",
                        pipe.name,
                        column_list(&fields),
                        column_list(&right_fields),
                    ),
                ));
            };
            if lk.ty != rk.ty {
                return Err(pipe_error(
                    db,
                    pipe,
                    format!(
                        "`join` in `{}` is on `.{key}`, and the two sides type it differently: \
                         {} against {}. There is no implicit coercion — a key compared across \
                         two types is a join whose result nobody declared",
                        pipe.name,
                        render_ty_kind(db, lk.ty.kind(db)),
                        render_ty_kind(db, rk.ty.kind(db)),
                    ),
                ));
            }

            let clash: Vec<&str> = right_fields
                .iter()
                .filter(|r| &r.name != key && fields.iter().any(|l| l.name == r.name))
                .map(|r| r.name.as_str())
                .collect();
            if !clash.is_empty() {
                return Err(pipe_error(
                    db,
                    pipe,
                    format!(
                        "`join` in `{}` would give one row two columns called {}. \
                         Only the key `.{key}` is identified by the join; every other name has \
                         to be distinct, so rename one side before joining",
                        pipe.name,
                        clash
                            .iter()
                            .map(|c| format!("`{c}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ));
            }

            let mut joined = fields.clone();
            joined.extend(right_fields.iter().filter(|r| &r.name != key).cloned());
            Ok(Ty::new(db, TyKind::Record(Record::new(db, joined))))
        }
    }
}

fn record_fields<'db>(db: &'db dyn fossil_base::Db, row: Ty<'db>) -> Option<Vec<RecordField<'db>>> {
    match row.kind(db) {
        TyKind::Record(rec) => Some(rec.fields(db).clone()),
        _ => None,
    }
}

fn column_list(fields: &[RecordField<'_>]) -> String {
    if fields.is_empty() {
        return "no columns at all".to_string();
    }
    fields
        .iter()
        .map(|f| format!("`{}`", f.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every `.column` an expression names, in order, duplicates included.
fn collect_field_refs(expr: &crate::lower::HirExpr, out: &mut Vec<SmolStr>) {
    use crate::lower::HirExpr;
    match expr {
        HirExpr::FieldRef(name) => out.push(name.clone()),
        HirExpr::ColumnRef { column, .. } => out.push(column.clone()),
        HirExpr::BinOp { lhs, rhs, .. } => {
            collect_field_refs(lhs, out);
            collect_field_refs(rhs, out);
        }
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => {
            collect_field_refs(cond, out);
            collect_field_refs(then, out);
            collect_field_refs(otherwise, out);
        }
        HirExpr::Call { args, .. } => {
            for a in args {
                collect_field_refs(a, out);
            }
        }
        // A subject IRI reads columns, and until the holes were parsed this
        // walker could not see a single one of them.
        HirExpr::Interpolation(parts) => {
            for p in parts {
                if let crate::lower::InterpolationPart::Hole(e) = p {
                    collect_field_refs(e, out);
                }
            }
        }
        HirExpr::StringLit(_) | HirExpr::PrefixedName { .. } | HirExpr::IntLit(_) => {}
    }
}

fn pipe_error(
    db: &dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    message: String,
) -> fossil_base::ErrorGuaranteed {
    delay_span_bug(db, Span::new(pipe.span.0, pipe.span.1), message)
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
        let prim = descriptor.type_for_column(&col.name).unwrap_or_else(|| {
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
        });
        let field_ty = Ty::new(db, TyKind::Primitive(prim));
        fields.push(RecordField {
            name: SmolStr::from(col.name.as_str()),
            ty: field_ty,
        });
    }
    let rec = Record::new(db, fields);
    Ty::new(db, TyKind::Record(rec))
}

/// Build a `Record` [`Ty`] from a `ShEx` shape — the COMPILE-TIME source row type
/// for an `io.rdf` destructuring member. Mirrors [`record_from_descriptor`]: one
/// field per shape constraint plus the always-present `subject` IRI column (the
/// pivoted RDF row carries the entity IRI there, so `iri = .subject` types).
///
/// Per-constraint typing mirrors the OUTPUT decomposition's literal-vs-shaperef
/// rule (`fossil-sinks::decomp::classify_object`): a literal `datatype` IRI maps
/// to its [`Primitive`] (via the XSD local name); a shape-ref / IRI-valued node
/// is an edge — its source-side value is the referenced subject's IRI, a String
/// column.
#[must_use]
pub(crate) fn record_from_shape<'db>(
    db: &'db dyn fossil_base::Db,
    desc: &fossil_descriptors_output::ShExDescriptor,
    shape_iri: &str,
) -> Ty<'db> {
    use fossil_descriptors_output::ConstraintValue;

    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    // Every pivoted RDF row carries its entity IRI in `subject`.
    let mut fields: Vec<RecordField<'db>> = vec![RecordField {
        name: SmolStr::new_static("subject"),
        ty: string_ty,
    }];
    if let Some(binding) = desc.lookup_shape_str(shape_iri) {
        for c in &binding.constraints {
            let prim = match c.value() {
                // An xsd type outside the lattice is a column all the same —
                // String is the conservative, column-producing choice.
                ConstraintValue::Datatype(iri) => {
                    Primitive::from_xsd_iri(&iri).unwrap_or(Primitive::String)
                }
                // Shape-ref / IRI-valued node → the referenced subject's IRI.
                ConstraintValue::Iri | ConstraintValue::Unknown => Primitive::String,
            };
            fields.push(RecordField {
                name: SmolStr::from(c.predicate_local_name()),
                ty: Ty::new(db, TyKind::Primitive(prim)),
            });
        }
    }
    Ty::new(db, TyKind::Record(Record::new(db, fields)))
}

// `resolve_relative` lived here as a byte-identical twin of `def_map`'s, with a
// comment conceding the duplication to avoid a cross-module `pub`. It is now
// `crate::def_map::resolve_relative`, imported above — a third reader made the
// trade stop paying.

/// Build a `Record` [`Ty`] from a host-provided [`InferredDescriptor`].
///
/// Phase 13 v0.2 (ADR-0037) inferred-path companion to
/// [`record_from_descriptor`]. The two functions produce structurally
/// equivalent Records on identical column shapes (semantic-equivalence
/// invariant from INPUT-03).
///
/// There is no unknown-datatype branch here any more: a column carries a
/// [`Primitive`], not the name of one, so a host that sends something outside
/// the lattice is rejected where its JSON is deserialised — before any of this
/// runs, and with the offending value in the error.
#[must_use]
pub(crate) fn record_from_inferred<'db>(
    db: &'db dyn fossil_base::Db,
    inferred: &InferredDescriptor,
) -> Ty<'db> {
    let fields: Vec<RecordField<'db>> = inferred
        .columns
        .iter()
        .map(|col| field_from_inferred(db, col))
        .collect();
    Ty::new(db, TyKind::Record(Record::new(db, fields)))
}

/// Emit the `D-CSVW-DEPRECATED` warning when a source binding's explicit
/// `schema = "..."` arg is encountered (ADR-0037).
///
/// Severity is conveyed via the `D-CSVW-DEPRECATED:` text prefix consumed
/// downstream by the diagnostic renderer; the underlying `delay_span_bug`
/// accumulator is the existing Phase-3 channel (a dedicated `warning`
/// accumulator is out-of-scope for plan 13-02).
fn emit_csvw_deprecated_diagnostic(db: &dyn fossil_base::Db, source_name: &SmolStr) {
    // FILE-ABSOLUTE, not mapping-relative: this is about the `SOURCE_DEF`'s
    // `schema = "..."` argument, which lives OUTSIDE any mapping. Marked so the
    // host's rebase leaves it alone — shifting it by the enclosing mapping's
    // start would point it at unrelated text.
    //
    // The span itself is still (0, 0): anchoring it on the `schema` argument
    // needs `def_map` to record that token's range, which it does not yet. So
    // this lands at the file head — imprecise, but honestly imprecise, which is
    // better than precisely wrong.
    fossil_base::Diagnostic::new(
        fossil_base::Severity::Error,
        format!(
            "D-CSVW-DEPRECATED: explicit CSVW descriptor for source \
             `{source_name}` is deprecated; types will be inferred from the \
             file directly. Remove the `schema = \"...\"` argument."
        ),
        Span::new(0, 0),
    )
    .file_absolute()
    .accumulate(db);
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
            uri: "users.csv".into(),
            columns: vec![
                InferredColumn {
                    name: "id".into(),
                    primitive: Primitive::Integer,
                },
                InferredColumn {
                    name: "name".into(),
                    primitive: Primitive::String,
                },
                InferredColumn {
                    name: "age".into(),
                    primitive: Primitive::Integer,
                },
            ],
            freshness_token: "test-token".into(),
        };
        let row_inferred = record_from_inferred(&db, &inferred);

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

    /// The unknown-primitive branch that used to live here is gone with the
    /// string: a non-lattice name no longer reaches the checker at all, it fails
    /// at the host boundary. `fossil-descriptors-input` owns that test now
    /// (`a_primitive_outside_the_lattice_is_a_deserialisation_error`).
    #[test]
    fn a_column_carries_the_lattice_and_not_its_spelling() {
        let db = db();
        let inferred = fossil_descriptors_input::InferredDescriptor {
            uri: "users.csv".into(),
            columns: vec![fossil_descriptors_input::InferredColumn {
                name: "born".into(),
                primitive: Primitive::GYear,
            }],
            freshness_token: String::new(),
        };
        let TyKind::Record(rec) = record_from_inferred(&db, &inferred).kind(&db) else {
            panic!("expected Record");
        };
        assert_eq!(
            rec.fields(&db)[0].ty.kind(&db),
            &TyKind::Primitive(Primitive::GYear),
            "the host's primitive arrives typed, with no name table in between"
        );
    }
}
