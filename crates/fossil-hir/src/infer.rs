//! Source row resolution: `MappingLoc` → `Option<Ty<'db>>` (a `Record` built
//! from a host-inferred descriptor or from a shape document).
//!
//! # Architectural constraint (Serious #6)
//!
//! This module reads the source's `schema =` argument from the
//! [`crate::def_map::DefMap`] (which carries signatures only, never a body),
//! NOT from the FILE CST via a `mapping_cst_node`-walk-upward. The latter would
//! reintroduce a `parse(db, file)` dependency inside the `typecheck_mapping` query,
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
//! Priority order:
//!
//! 1. **`InferredDescriptor`** (preferred). When the host has pre-registered
//!    a descriptor for the mapping's source **URI** via
//!    `db.system().descriptors()` (browser-side `DuckDB-WASM` via
//!    `FossilPlayground::registerInferredDescriptor`; the native engine via
//!    the `duckdb` crate), `resolve_source_scope` consumes that descriptor and
//!    builds the [`Record`] directly — nothing is read from disk.
//!
//!    The lookup key is the URI as the program writes it, not the binding
//!    name: the descriptor describes a file, and two bindings may
//!    name one. A source whose RHS is not a recognisable `io.*("…")` call has
//!    no URI, hence no descriptor, and falls through to 2 or 3.
//!
//! 2. **A shape document**, for an `io.rdf` destructuring member: the shape
//!    the `schema = io.shex("…")` argument names IS the row.
//!
//! 3. **No descriptor**. Returns `None`; forward propagation is disabled for
//!    the mapping — `.field` accesses synthesise no type.
//!
//! ## Salsa-safety of the inferred path
//!
//! `db.system().descriptors()` reads through the existing `System`
//! abstraction — the one indirection a host is swapped behind. The descriptor
//! table is NOT a `salsa::input` — it is host-owned state on the System impl,
//! mirroring
//! `read_file`. Reads from inside a tracked query do not register a Salsa
//! input dependency, so re-registering a descriptor does NOT trigger
//! invalidation. (When the host wants to invalidate, it bumps the source
//! file's text via `set_text`, which Salsa already tracks.)

use fossil_base::{Span, delay_span_bug};
use fossil_descriptors_input::InferredDescriptor;
use smol_str::SmolStr;

use fossil_graph_schema::{Primitive, Shape, local_name};

use crate::def_map::{MappingLoc, ShapeBindError, def_map};
use crate::ty::{Record, RecordField, Rows, Ty, TyKind};

/// The source-row [`Ty`] (a `Record`) for a mapping as known from the
/// host-registered [`fossil_descriptors_input::InferredDescriptor`] ONLY.
///
/// Side-effect-free: no diagnostics, no filesystem reads — the
/// descriptor-branch of [`resolve_source_scope`] without the type-check's
/// `delay_span_bug` emission. For IDE features (completion, hover) that want
/// the source schema OUTSIDE a tracked query. Returns `None` when no descriptor
/// is registered for the mapping's source URI — the host did not pre-introspect.
#[must_use]
pub fn source_row_inferred<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<Ty<'db>> {
    let file = mapping.file(db);
    let mappings = crate::lower::lower_to_hir(db, file);
    let hir_mapping = mappings.mapping(db, mapping.index(db))?;
    let source_name = hir_mapping.source_binding.clone();
    let inferred = lookup_inferred(db, def_map(db, file), source_name.as_str())?;
    Some(record_from_inferred(db, &inferred))
}

/// The descriptor registered for the URI `source_name` is bound to, if the
/// binding has a URI at all and the host keeps a table.
///
/// The indirection binding-name → URI → descriptor is what the cache being
/// keyed by URI costs a consumer, and it costs nothing: the descriptor
/// describes a FILE, so the key is the URI the program writes and never the
/// binding's name, and the `DefMap` already carries that URI (it has
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

// `RowScope` lived here — the rows of a relation, beside the type system rather
// than in it. It is `crate::ty::Rows` now, the payload of `TyKind::Relation`,
// and this module keeps the ALGEBRA that builds one.
//
// Two structures described the shape of data in flight and only one of them was
// a type, which is why `seq.where` had the signature `p("rows", S::String)` and
// why a `where` predicate was walked for the columns it names and never typed.

/// The scope a mapping's `from` clause puts in the body — see [`Rows`].
///
/// Plain-Rust helper (NOT `#[salsa::tracked]`) — called from within the
/// `typecheck_mapping` tracked query so its `delay_span_bug` emits are valid.
///
/// Reads `def_map(db, file)` only (NEVER walks the FILE CST from
/// `mapping_cst_node`) — see the module docs for the Serious #6 rationale.
pub fn resolve_source_scope<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Result<Option<Rows<'db>>, fossil_base::ErrorGuaranteed> {
    let file = mapping.file(db);

    // 1. Find the mapping's source binding name.
    let mappings = crate::lower::lower_to_hir(db, file);
    let Some(hir_mapping) = mappings.mapping(db, mapping.index(db)) else {
        return Ok(None);
    };
    let source_name = hir_mapping.source_binding.clone();

    resolve_binding_scope(db, file, source_name.as_str(), 0).map(Some)
}

/// A source pipeline that derives from a pipeline that derives from … Thirty-two
/// is not a limit anyone will meet writing a mapping; it is the depth at which a
/// CYCLE (`a := b.where(…)`, `b := a.where(…)`) stops being an infinite
/// loop and becomes a diagnostic.
const MAX_PIPE_DEPTH: usize = 32;

/// The row type of a source BINDING, by name — [`resolve_binding_scope`]
/// flattened. `None` when the binding declares no schema.
pub fn resolve_binding_row<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    source_name: &str,
    depth: usize,
) -> Result<Option<Ty<'db>>, fossil_base::ErrorGuaranteed> {
    Ok(resolve_binding_scope(db, file, source_name, depth)?.flat(db))
}

/// The [`Rows`] of a source BINDING, by name.
///
/// Split out of [`resolve_source_scope`] because a pipeline's row is its base's
/// row transformed, and the base is a binding, not a mapping. Everything below
/// the pipeline branch is what the function has always done for a binding that
/// reads a file — and it is where the scope BOTTOMS OUT at one row under one
/// name. Every verb but `union` carries, restricts or extends the names its
/// base already had; `union` replaces them with the pipeline's own, because a
/// row of its result came from one of two sides and nothing says which.
pub fn resolve_binding_scope<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    source_name: &str,
    depth: usize,
) -> Result<Rows<'db>, fossil_base::ErrorGuaranteed> {
    let dm = def_map(db, file);

    // A binding whose right-hand side is a pipeline: the row is the base's row
    // put through the verbs. A binding with no descriptor still has
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
        // The NAMES survive it, which is the whole reason this returns a scope:
        // `Contact.email` has to resolve against `Reachable := Contact.where(…)`
        // whether or not anybody registered a descriptor for `Contact`.
        let mut scope = resolve_binding_scope(db, file, pipe.base.as_str(), depth + 1)?;
        for op in &pipe.ops {
            scope = apply_source_op(db, file, pipe, op, scope, depth)?;
        }
        return Ok(scope);
    }

    // Not derived from anything: the binding READS something, and it is the one
    // name its own row answers to.
    Ok(Rows::one(
        source_name,
        resolve_leaf_row(db, file, dm, source_name)?,
    ))
}

/// The row of a binding that reads a file — the bottom of every scope.
///
/// Lifted out of [`resolve_binding_scope`] unchanged when the scope arrived:
/// every branch below answers the same question it always did, about ONE
/// binding, and the name it answers under is the caller's business.
// No branch here errors TODAY, and the `Result` stays: this is one of four
// functions in this module with the same signature, called with `?` from the
// scope walk, and the taint an `ErrorGuaranteed` carries is the thing that must
// be able to propagate from any of them.
#[allow(clippy::unnecessary_wraps)]
fn resolve_leaf_row<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    dm: crate::def_map::DefMap<'db>,
    source_name: &str,
) -> Result<Option<Ty<'db>>, fossil_base::ErrorGuaranteed> {
    // Try the host-registered `InferredDescriptor` FIRST. This is the new authoring style — the user writes `io.csv("...")`
    // and the host (browser-side `DuckDB-WASM`; native CLI `duckdb` crate)
    // pre-registers the descriptor before invoking `compile`.
    if let Some(inferred) = lookup_inferred(db, dm, source_name) {
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
        // The PAIR: `schema = io.shex("x.shex")` names the row that reads the
        // document as well as the document, and passing `None` here made every
        // destructuring RDF source report "named by no provider".
        let Some((provider, schema_path)) = dm.lookup_source_schema_binding(db, source_name) else {
            return Ok(None);
        };
        let document = match crate::shapes::decoded_document(
            db,
            file,
            provider.as_deref(),
            schema_path.as_str(),
        ) {
            Ok(d) => d,
            Err(e) => {
                let _eg = delay_span_bug(
                    db,
                    Span::new(0, 0),
                    format!(
                        "the shape document `{schema_path}` for source \
                         `{source_name}` gave nothing to type against: {e}"
                    ),
                );
                return Ok(None);
            }
        };
        // `shape_iri` came out of THIS document, by position, in `def_map` —
        // same path, same decoder, same query. A miss here is not a user error
        // and there is no message that would help one, so it is reported as
        // what it is.
        let Some(shape) = document.lookup(shape_iri.as_str()) else {
            let _eg = delay_span_bug(
                db,
                Span::new(0, 0),
                format!(
                    "internal: `{schema_path}` bound `{source_name}` to shape \
                     `{shape_iri}` and no longer declares it"
                ),
            );
            return Ok(None);
        };
        return Ok(Some(record_from_shape(db, shape)));
    }

    // A destructuring member that bound no shape, reported by CAUSE. One
    // message used to cover all four and blamed the member's name for every
    // one of them, so an unreadable file read as a misspelt name. Binding is
    // positional now — the Nth name takes the Nth shape the document declares —
    // so "matches no shape" is not among the causes any more: the name is a
    // free local label and selects nothing.
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
            // `position` is not read: this sentence names the source binding
            // rather than a number, so the surplus member's index has nothing
            // to say in it.
            ShapeBindError::Arity {
                declared, named, ..
            } => format!(
                "the binding names {named} shape(s) and the document declares {declared}; \
                 names bind by position, so `{source_name}` has no shape to bind"
            ),
        };
        let _eg = delay_span_bug(db, Span::new(0, 0), message);
        return Ok(None);
    }

    // There is no fallback below this: a source with no INFERRED descriptor has
    // no row, and that is not an error — it is every schemaless program in the
    // tree. The host introspects
    // (`fossil_introspect::pre_introspect_and_register`, the browser's
    // `registerInferredDescriptor`), and what it finds arrives above.
    Ok(None)
}

/// One verb of a source pipeline applied to the [`Rows`] it receives.
///
/// Every refusal names the pipeline and the columns it actually has: a row
/// algebra whose errors say "column not found" and stop is a row algebra nobody
/// can debug from the message.
///
/// It takes a scope and not a `Ty`, because a `join` has to hand on two sides
/// that stay addressable under their own binding names, and a flat `Record`
/// cannot carry that.
fn apply_source_op<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    pipe: &crate::lower::HirSourcePipe,
    op: &crate::lower::HirSourceOp,
    scope: Rows<'db>,
    depth: usize,
) -> Result<Rows<'db>, fossil_base::ErrorGuaranteed> {
    use crate::lower::HirSourceOp;

    // An untyped input is not an error — it is every schemaless program in the
    // tree — and nothing below can check a column against a row nobody
    // declared. The NAMES still flow, so the scope is returned rather than
    // dropped.
    // The COLUMNS are no longer read here — `typecheck_stage` asks the scope.
    // What this still is, is the question «does this relation have a schema at
    // all», and the answer decides whether there is anything to check.
    let Some(_) = scope.fields(db) else {
        return match op {
            // A join still has to bring the right-hand names in, or a body that
            // writes `User.email` next to an untyped `Purchase` would be told
            // `User` is not a row this mapping has — which is false, and is the
            // diagnostic this whole seam exists to stop being wrong.
            //
            // The condition's SHAPE is checked here too, and that is the whole
            // reason this arm names `on` at all. Whether the two sides are
            // related is a question about bindings and operators, and neither
            // needs a column type to answer — so a schemaless join, which is
            // `hello.fossil` and every program like it, is where the engine's
            // late refusal used to be the only one there was.
            HirSourceOp::Join { right, alias, on } => {
                let right = right_scope(db, file, right, alias.as_ref(), depth)?;
                check_join_condition(db, pipe, on, &scope, &right)?;
                Ok(scope.concat(right))
            }
            // A union answers to ONE name whether or not either side has a
            // schema, and the two sides' names do not survive it: the pipeline
            // is the only thing a body can address the result by.
            HirSourceOp::Union { .. } => Ok(scope.rename_to(db, &pipe.name)),
            // A `group_by` REPLACES the row whether or not the input declared
            // one: what survives is the keys and the aggregates, and an
            // aggregate is a column no source ever had.
            HirSourceOp::GroupBy { keys, aggs } => Ok(grouped_scope(db, pipe, &scope, keys, aggs)),
            HirSourceOp::Where(_) | HirSourceOp::Select(_) | HirSourceOp::Distinct => Ok(scope),
        };
    };
    match op {
        // `where` keeps rows, not columns: the row type is its input's. The
        // columns the predicate names still have to exist — a filter on a column
        // that is not there is a program that would run and keep everything.
        HirSourceOp::Where(pred) => {
            typecheck_stage(db, file, pipe, "seq.where", pred, &scope)?;
            Ok(scope)
        }
        // `select` restricts, and it restricts EACH ROW: `Employee.id` stays a
        // column of `Employee` after `Active.select(Employee.id, …)`, because
        // the binding is what a body writes. The payload CARRIES the binding —
        // `HirSourceOp::Select` holds `SelectedColumn`s — so the row is chosen
        // by the name the author wrote and the column is looked for in that row
        // alone. A bare name looked up in the rows in order would make
        // `select(id)` mean the left side's after a join.
        //
        // There are therefore two refusals here and not one: an unknown BINDING
        // and an unknown COLUMN of a known binding are different mistakes, and a
        // single "its input does not have it" cannot say which.
        HirSourceOp::Select(cols) => {
            let mut kept: Vec<(SmolStr, Vec<RecordField<'db>>)> =
                scope.bindings().map(|b| (b.clone(), Vec::new())).collect();
            for col in cols {
                let f = column_of(db, pipe, &scope, col, "select")?;
                if let Some((_, out)) = kept.iter_mut().find(|(b, _)| b == &col.binding) {
                    out.push(f);
                }
            }
            Ok(Rows::of(
                kept.into_iter()
                    .map(|(binding, fs)| crate::ty::NamedRow {
                        binding,
                        row: Some(Ty::new(db, TyKind::Record(Record::new(db, fs)))),
                    })
                    .collect(),
            ))
        }
        // The join does NOT flatten, and there is no name-collision rule: the
        // body writes `Purchase.amount` and `User.email`, so the two sides stay
        // two entries and a shared column name means nothing. `Purchase.id` and
        // `User.id` land on different entries; only a BARE name — which the
        // surface has no spelling for — would flatten into finding the first.
        HirSourceOp::Join { right, alias, on } => {
            let right = right_scope(db, file, right, alias.as_ref(), depth)?;
            let Some(_) = right.fields(db) else {
                return Err(pipe_error(
                    db,
                    pipe,
                    format!(
                        "`join` in `{}` joins `{}`, whose columns are unknown — it declares \
                         no schema, so there is nothing to check the condition against",
                        pipe.name,
                        right
                            .bindings()
                            .map(SmolStr::as_str)
                            .collect::<Vec<_>>()
                            .join("`, `"),
                    ),
                ));
            };

            // Every column the condition names has to exist on ONE of the two
            // sides, under the binding that qualifies it. It used to have to
            // exist on BOTH, because there was one key; a predicate relates two
            // different columns, so "both" is the wrong question. And "on either
            // side, under any name" is not the right one either — that is what
            // let `on = Purchase.user_id == Nobody.id` through while `Nobody`
            // named nothing at all.
            // The two sides' columns were concatenated into one flat list here
            // and handed to `check_refs`, which resolved a name against it. The
            // scope IS that answer and keeps the sides apart, so the flat list
            // went with the walk.
            // Two questions, and the order is the useful one. `typecheck_stage`
            // answers «is this a condition at all, and do its columns exist»;
            // `check_join_condition` answers «and does it relate THESE two
            // sides». Typing first means a misspelled column is reported as a
            // misspelled column and not as a side that goes unmentioned.
            let joined = scope.clone().concat(right.clone());
            typecheck_stage(db, file, pipe, "seq.join", on, &joined)?;
            check_join_condition(db, pipe, on, &scope, &right)?;
            Ok(joined)
        }
        // `distinct` keeps rows, not columns, and names none of them: the row
        // type is its input's, and there is nothing to check.
        HirSourceOp::Distinct => Ok(scope),
        // The one verb that puts a column no source declared into a relation,
        // and the whole of why it is the expensive one. Two refusals, and they
        // are `select`'s two, because a key and an aggregated column are both
        // qualified names of the input: an unknown BINDING and an unknown
        // COLUMN of a known binding are different mistakes.
        HirSourceOp::GroupBy { keys, aggs } => {
            for col in keys.iter().chain(aggs.iter().map(|a| &a.column)) {
                column_of(db, pipe, &scope, col, "group_by")?;
            }
            Ok(grouped_scope(db, pipe, &scope, keys, aggs))
        }
        // The one verb whose two inputs must agree rather than combine.
        // `Op::Union`'s schema rule is `left`, asserted equal to `right`, and
        // `fossil-df` unions BY POSITION — so equal names in equal order with
        // equal types is not a stricter rule than the backend's, it is the
        // backend's rule stated where a user can be told about it.
        HirSourceOp::Union { right } => {
            let right = right_scope(db, file, right, None, depth)?;
            let (Some(left_fields), Some(right_fields)) = (scope.fields(db), right.fields(db))
            else {
                // The right side declares no schema. Nothing can be compared,
                // and a pipeline over an untyped source is every schemaless
                // program in the tree — so the result is untyped too, under the
                // one name it answers to.
                return Ok(scope.rename_to(db, &pipe.name));
            };
            if left_fields.len() != right_fields.len() {
                return Err(pipe_error(
                    db,
                    pipe,
                    format!(
                        "`union` in `{}` needs both sides to carry the same row. The left has \
                         {}; the right has {}.",
                        pipe.name,
                        column_list(&left_fields),
                        column_list(&right_fields),
                    ),
                ));
            }
            for (i, (l, r)) in left_fields.iter().zip(right_fields.iter()).enumerate() {
                if l.name != r.name || l.ty != r.ty {
                    return Err(pipe_error(
                        db,
                        pipe,
                        format!(
                            "`union` in `{}` pairs its sides column by column, and column {} is \
                             `{}` ({}) on the left and `{}` ({}) on the right.",
                            pipe.name,
                            i + 1,
                            l.name,
                            crate::ty::display::render_ty_kind(db, l.ty.kind(db)),
                            r.name,
                            crate::ty::display::render_ty_kind(db, r.ty.kind(db)),
                        ),
                    ));
                }
            }
            Ok(scope.rename_to(db, &pipe.name))
        }
    }
}

/// One qualified column of a scope, or the refusal that says which half is
/// wrong.
///
/// Two refusals and not one: an unknown BINDING and an unknown COLUMN of a
/// known binding are different mistakes, and a single "its input does not have
/// it" cannot say which. `verb` is the word the message opens with, because
/// `select` and `group_by` ask this same question of the same scope.
fn column_of<'db>(
    db: &'db dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    scope: &Rows<'db>,
    col: &crate::lower::SelectedColumn,
    verb: &str,
) -> Result<RecordField<'db>, fossil_base::ErrorGuaranteed> {
    let (binding, column) = (&col.binding, &col.column);
    let Some(fields) = scope.fields_of(db, binding) else {
        return Err(pipe_error(
            db,
            pipe,
            format!(
                "`{verb}` in `{}` names `{binding}.{column}`, and `{}` carries no row called \
                 `{binding}`. It draws on: {}",
                pipe.name,
                pipe.name,
                scope
                    .bindings()
                    .map(|b| format!("`{b}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ));
    };
    fields
        .iter()
        .find(|f| &f.name == column)
        .cloned()
        .ok_or_else(|| {
            pipe_error(
                db,
                pipe,
                format!(
                    "`{verb}` in `{}` names `{binding}.{column}`, which `{binding}` does not \
                     have. It has: {}",
                    pipe.name,
                    column_list(&fields),
                ),
            )
        })
}

/// The rows a `group_by` hands on: the keys under the bindings that own them,
/// and the aggregates under the PIPELINE's name.
///
/// The aggregates answer to the pipeline for the reason a `union`'s row does —
/// a group's total belongs to no side, so there is nothing to qualify it with.
/// The keys are not touched: each one is still a column of the source that
/// declared it, which is what the body writes.
///
/// Nothing else of the input survives. A column that is neither grouped nor
/// aggregated has one value per row where the result has one per group, so
/// there is nothing for it to be.
fn grouped_scope<'db>(
    db: &'db dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    scope: &Rows<'db>,
    keys: &[crate::lower::SelectedColumn],
    aggs: &[crate::lower::HirAggregation],
) -> Rows<'db> {
    let reg = crate::stdlib::stdlib();
    let mut rows: Vec<crate::ty::NamedRow<'db>> = Vec::new();
    for binding in scope.bindings() {
        let fields: Vec<RecordField<'db>> = keys
            .iter()
            .filter(|k| &k.binding == binding)
            .filter_map(|k| {
                scope
                    .fields_of(db, binding)
                    .and_then(|fs| fs.into_iter().find(|f| f.name == k.column))
            })
            .collect();
        rows.push(crate::ty::NamedRow {
            binding: binding.clone(),
            row: Some(Ty::new(db, TyKind::Record(Record::new(db, fields)))),
        });
    }
    // The aggregate's type is the ROW's return type and not the column's:
    // `math.sum` takes a `Float` and gives one, so summing an `Integer` column
    // produces a `Float`. That is the type change a checker has to make and a
    // syntax cannot.
    let agg_fields: Vec<RecordField<'db>> = aggs
        .iter()
        .filter_map(|a| {
            let out_ty = reg.lookup(a.func.as_str())?.sig.ret.scalar()?;
            Some(RecordField {
                name: a.out.clone(),
                ty: out_ty.to_ty(db),
            })
        })
        .collect();
    rows.push(crate::ty::NamedRow {
        binding: pipe.name.clone(),
        row: Some(Ty::new(db, TyKind::Record(Record::new(db, agg_fields)))),
    });
    Rows::of(rows)
}

/// The right-hand side of a join, under the name the body will address it by.
///
/// `Node.join(Node as Other, …)`: the alias is the second name for the same
/// source and it is the only thing that can tell the two sides apart, so it
/// REPLACES the right side's own scope rather than being added beside it. Both
/// spellings would be a way to name one row twice.
fn right_scope<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    right: &SmolStr,
    alias: Option<&SmolStr>,
    depth: usize,
) -> Result<Rows<'db>, fossil_base::ErrorGuaranteed> {
    let scope = resolve_binding_scope(db, file, right.as_str(), depth + 1)?;
    Ok(match alias {
        Some(a) => scope.rename_to(db, a),
        None => scope,
    })
}

// `check_refs`, `collect_refs`, `Reference` and `binding_list` stood here — a
// hand-written walk that collected the `ColumnRef`s a condition mentions and
// asked, per name, whether the scope had it. Every one of those questions is a
// question `synth` of a `ColumnRef` already asks, against the same scope. What
// the walk could NOT ask is the other one, and that is the whole finding: it
// answered «is there a column called `celsius`» and never «and is
// `Row.celsius > "abc"` a condition».

/// Type one stage's condition, against the rows the pipeline has AT that stage.
///
/// The catalogue row says what the verb takes — `seq.where` is
/// `(Rows, Predicate) -> Rows` — and a [`crate::check::Expr`] over `scope`
/// answers what the condition IS. That is the whole of it: an expression is
/// typed the same way wherever it is written.
///
/// It replaces `check_refs`, a hand-written walk that collected the
/// `ColumnRef`s a condition mentions and asked whether each name existed. Every
/// one of those questions is one `synth` of a `ColumnRef` already asks, with
/// the same scope — and the walk could not ask the other one, so
/// `Row.celsius > "abc"` passed clean two lines above a
/// `parse.float(Row.celsius)` refused for the same mismatch.
///
/// The scope AT THIS STAGE and not the pipeline's final one: a `select` after a
/// `where` narrows the row, and checking the predicate against what the
/// pipeline ends up with would check it against a row it never saw.
fn typecheck_stage<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    pipe: &crate::lower::HirSourcePipe,
    verb: &str,
    condition: &crate::lower::HirExpr,
    scope: &Rows<'db>,
) -> Result<(), fossil_base::ErrorGuaranteed> {
    let at = pipe.span;
    let mut cx = crate::check::Expr::over_relation(db, file, pipe.base.clone(), scope.clone(), at);
    let ty = cx.synth(crate::body::ExprId(0), condition);

    // The signature is what says a condition is wanted here, so it is what the
    // refusal rests on. A `seq/` row that lost its `Predicate` parameter is a
    // catalogue bug, and `crate::stdlib::tests` is where that is caught.
    let wants_predicate = crate::stdlib::stdlib().lookup(verb).is_some_and(|e| {
        e.sig
            .params
            .iter()
            .any(|p| p.ty == crate::stdlib::SigTy::Predicate)
    });
    if let Some(ty) = ty
        && wants_predicate
        && !matches!(ty.kind(db), TyKind::Error(_))
        && !crate::check::subtypes(db, ty, Ty::new(db, TyKind::Primitive(Primitive::Bool)))
    {
        let d = fossil_base::Diagnostic::new(
            fossil_base::Severity::Error,
            format!(
                "`{}` in `{}` needs a condition, and this is {}",
                crate::stdlib::split_receiver(verb).1,
                pipe.name,
                crate::ty::display::render_ty_kind(db, ty.kind(db)),
            ),
            at,
        )
        .file_absolute();
        return Err(fossil_base::raise(db, d));
    }

    cx.first_error.map_or(Ok(()), Err)
}

/// Which side of a join one column reference names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConditionSide {
    Left,
    Right,
}

/// The SHAPE of a join condition, exacted where the join is WRITTEN.
///
/// [`typecheck_stage`] answers the first half — the condition is a `Bool`, and
/// every column it names exists under the binding that qualifies it. That is
/// not enough to be a join. `Purchase.join(User, on = Purchase.id ==
/// Purchase.user_id)` passes both of those questions and never mentions `User`,
/// so what it describes is every order paired with every user and then
/// filtered. So does `Both.join(Third, on = Left.k == Right.k)`, where both
/// bindings are real, both belong to the LEFT side, and `Third` is joined to
/// everything.
///
/// # The rule
///
/// The condition is a conjunction of equalities, and each equality equates a
/// column of one side with a column of the other. Three refusals fall out, and
/// each is a distinct mistake with a message of its own: a conjunct that is not
/// an `==`, an operand that is not a bare column reference, and an equality
/// whose two operands come from the same side.
///
/// # Why here and not one layer down
///
/// The rule is not new: `fossil_df::plan::join_equalities` has enforced it since
/// the day joins executed, and refused a malformed condition with a
/// `DataFusionError::Plan` at `fossil run` — late, and with no span, because by
/// then the author's text is gone. The engine keeps its copy as a backstop: a
/// MIR reaching it need not have come through this checker.
///
/// What the checker can say that the engine cannot is WHICH SIDE, in the
/// author's own names. A `JoinSide` carries one relation name — the pipeline's
/// after a previous join — while the scope here carries every binding a body
/// may address, which is what makes «and `Third` goes unmentioned» sayable at
/// all.
///
/// # What is refused that a relational algebra would allow
///
/// A theta join (`on = A.since < B.at`) and a disjunction (`on = A.k == B.k or
/// A.j == B.j`) are ordinary relational conditions and are refused here, for the
/// engine's reason: neither leaves an equality to plan a hash join against, so
/// what executes is a nested loop over the product of two corpora, and a corpus
/// is the size where that is not a slow program but a hung one.
///
/// A COMPUTED key (`on = str.lower(A.k) == B.k`) is refused with them, and it is
/// the one refusal with no workaround inside the language today: `seq.map` takes
/// no function (`grammar.bnf` lists `LambdaExpr` among the productions
/// intentionally absent) and there is no `extend`, so there is nowhere to put
/// the computation. Widening the rule to admit it would
/// mean admitting an arbitrary expression as a key, which is the theta join
/// again wearing an `==`.
fn check_join_condition<'db>(
    db: &'db dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    on: &crate::lower::HirExpr,
    left: &Rows<'db>,
    right: &Rows<'db>,
) -> Result<(), fossil_base::ErrorGuaranteed> {
    use crate::display::expr_text;
    use crate::lower::{BinOp, HirExpr};

    for conjunct in conjuncts(on) {
        let HirExpr::BinOp {
            op: BinOp::Eq,
            lhs,
            rhs,
        } = conjunct
        else {
            return Err(join_error(
                db,
                pipe,
                format!(
                    "`join` in `{}` relates its two sides by equality, and `{}` is not one",
                    pipe.name,
                    expr_text(conjunct),
                ),
                "`on` takes an equality between one column of each side — `on = A.k == B.k` — or \
                 several of them joined by `and`. Anything else leaves no key to plan against, and \
                 what it would run as is a nested loop over the product of the two relations.",
            ));
        };

        let mut sides = Vec::with_capacity(2);
        for operand in [lhs.as_ref(), rhs.as_ref()] {
            let HirExpr::ColumnRef { binding, column } = operand else {
                return Err(join_error(
                    db,
                    pipe,
                    format!(
                        "`join` in `{}` equates `{}` with `{}`, and `{}` is not a column reference",
                        pipe.name,
                        expr_text(lhs),
                        expr_text(rhs),
                        expr_text(operand),
                    ),
                    "A join key is a column of one of the two sides, written `Binding.column`, and \
                     not a literal or anything computed from a column. There is nowhere to compute \
                     one first — `map` takes no function and the language has no `extend` — so a \
                     derived key has to be prepared in the source.",
                ));
            };
            let Some(side) = side_of(left, right, binding) else {
                return Err(join_error(
                    db,
                    pipe,
                    format!(
                        "`join` in `{}` names `{binding}.{column}`, and `{binding}` is neither \
                         side of this join",
                        pipe.name,
                    ),
                    &format!(
                        "The left side has {}; the right side has {}.",
                        binding_list(left),
                        binding_list(right),
                    ),
                ));
            };
            sides.push(side);
        }

        if sides[0] == sides[1] {
            let (named, unmentioned) = if sides[0] == ConditionSide::Left {
                ("left", right)
            } else {
                ("right", left)
            };
            return Err(join_error(
                db,
                pipe,
                format!(
                    "`join` in `{}` equates `{}` with `{}`, and both are columns of the {named} \
                     side",
                    pipe.name,
                    expr_text(lhs),
                    expr_text(rhs),
                ),
                &format!(
                    "A condition that never names {} holds or fails for whole relations rather \
                     than for a pairing, so every row of one side pairs with every row of the \
                     other. That is a filter over their product, not a join.",
                    binding_list(unmentioned),
                ),
            ));
        }
    }
    Ok(())
}

/// Split a condition on `and`. A conjunction is the only structure a join
/// condition is taken apart by; everything else is a leaf for
/// [`check_join_condition`] to admit or refuse.
///
/// `fossil_df::plan::conjuncts` is the same function over `fossil_mir::Expr`,
/// and the two cannot be one: the HIR is where the author's names still are, and
/// this crate is below the one that owns the MIR.
fn conjuncts(on: &crate::lower::HirExpr) -> Vec<&crate::lower::HirExpr> {
    match on {
        crate::lower::HirExpr::BinOp {
            op: crate::lower::BinOp::And,
            lhs,
            rhs,
        } => {
            let mut out = conjuncts(lhs);
            out.extend(conjuncts(rhs));
            out
        }
        other => vec![other],
    }
}

/// The side of the join a binding belongs to, or `None` when it belongs to
/// neither.
///
/// The left scope is searched first, and after a self-join that ordering is the
/// answer rather than a tiebreak: `Node.join(Node as Other, …)` renames the
/// right side to `Other`, so `Node` is on the left and only there. A join whose
/// right side kept its own name AND appeared on the left would be a self-join
/// without an alias, which the scope cannot represent.
fn side_of(left: &Rows<'_>, right: &Rows<'_>, binding: &str) -> Option<ConditionSide> {
    if left.has(binding) {
        Some(ConditionSide::Left)
    } else if right.has(binding) {
        Some(ConditionSide::Right)
    } else {
        None
    }
}

/// The bindings of one side of a join, for a message that has to name them.
fn binding_list(rows: &Rows<'_>) -> String {
    let names: Vec<String> = rows.bindings().map(|b| format!("`{b}`")).collect();
    match names.len() {
        0 => "no rows at all".to_string(),
        _ => names.join(", "),
    }
}

/// A refusal about the join condition, pointed at the pipeline that wrote it.
///
/// File-absolute for the reason every pipeline diagnostic is: a pipeline's span
/// is measured against the file and the default frame is `MappingRelative`.
fn join_error(
    db: &dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    message: String,
    help: &str,
) -> fossil_base::ErrorGuaranteed {
    let d = fossil_base::Diagnostic::new(fossil_base::Severity::Error, message, pipe.span)
        .with_help(help)
        .file_absolute();
    fossil_base::raise(db, d)
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

fn pipe_error(
    db: &dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    message: String,
) -> fossil_base::ErrorGuaranteed {
    delay_span_bug(db, pipe.span, message)
}

/// Build a `Record` [`Ty`] from a decoded [`Shape`] — the COMPILE-TIME source
/// row type for an `io.rdf` destructuring member. One field per constraint plus
/// the always-present
/// `subject` IRI column (the pivoted RDF row carries the entity IRI there, so
/// `iri = .subject` types).
///
/// Two rules, and they are the same two the output side applies in
/// [`fossil_graph_schema::OutputShapes::to_graph_schema`], which branches on
/// the same `targets` field:
///
/// - A constraint with **targets** is an edge, and the source-side value of an
///   edge is the referenced subject's IRI — so the column is [`TyKind::Iri`].
/// - Anything else takes its declared datatype, and a constraint the document
///   did not narrow is a `String` column. That is the permissive
///   walking-skeleton default, and it is the same one
///   [`fossil_graph_schema::OutputShapes::to_graph_schema`] applies, so the row
///   the checker sees and the column the writer emits cannot disagree.
///
/// # Why the edge column is `Iri` and not `String`
///
/// It used to be `String`, with this very doc-comment calling it "the referenced
/// subject's IRI" in the same sentence. Three places then contradicted each
/// other: [`crate::shapes::expected_value_ty`] demands `Iri` of a predicate
/// whose range is a shape, this row supplied `String`, and
/// [`crate::check`]'s `subtypes` has no rule between them — so
/// `hasProject = .hasProject`, copying an edge straight from an RDF input to an
/// RDF output, was **unsatisfiable**: "expected `Iri`, got `String`". And a type
/// error is not informational — it taints `typecheck_mapping`, which poisons
/// `lower_to_mir_pg`, which fails the run.
///
/// The lie was here. `expected_value_ty`'s own reasoning is sound — *the only
/// thing a reference can be is an IRI* — so the fix is the row that knew it was
/// handing back an IRI and typed it as text. Nothing downstream of the writer
/// moves: `fossil_df` derives the physical column with
/// `inner_primitive(...).unwrap_or(Primitive::String)`, and `Iri` has no inner
/// primitive, so the column is still `string`. What changes is that the checker
/// can tell an edge column from a text column — the same argument the `AnyUri`
/// row of `a_shape_becomes_a_source_row_of_subject_plus_one_column_per_constraint`
/// already makes for opaque IRIs.
///
/// The `subject` column moves with it, and for the same reason: it holds the
/// pivoted row's entity IRI, so `@subject = User.subject` is an identity built
/// from an IRI rather than from text.
///
/// The field name is the predicate's local name — the last segment of its IRI —
/// taken from the one [`local_name`] the vocabulary owns rather than a fourth
/// copy of it.
#[must_use]
pub(crate) fn record_from_shape<'db>(db: &'db dyn fossil_base::Db, shape: &Shape) -> Ty<'db> {
    // Every pivoted RDF row carries its entity IRI in `subject`, and that IRI
    // identifies a node of THIS shape — so the column is a reference to it,
    // where it used to be the untyped `TyKind::Iri` every reference shared.
    let mut fields: Vec<RecordField<'db>> = vec![RecordField {
        name: SmolStr::new_static("subject"),
        ty: Ty::reference(db, std::iter::once(SmolStr::from(shape.iri.as_str()))),
    }];
    for c in &shape.properties {
        let ty = if c.targets.is_empty() {
            Ty::new(
                db,
                TyKind::Primitive(c.datatype.unwrap_or(Primitive::String)),
            )
        } else {
            Ty::reference(db, c.targets.iter().map(SmolStr::from))
        };
        fields.push(RecordField {
            name: SmolStr::from(local_name(&c.predicate)),
            ty,
        });
    }
    Ty::new(db, TyKind::Record(Record::new(db, fields)))
}

// `resolve_relative` lived here as a byte-identical twin of `def_map`'s, with a
// comment conceding the duplication to avoid a cross-module `pub`. It is now
// `crate::def_map::resolve_relative`, imported above — a third reader made the
// trade stop paying.

/// Build a `Record` [`Ty`] from a host-provided [`InferredDescriptor`].
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    /// A one-line rendering of a scope: `Contact{id,email} + Other{id}`.
    ///
    /// The binding NAMES are what these tests are about, so they are in the
    /// string and not behind an accessor call per assertion.
    fn render(db: &dyn fossil_base::Db, scope: &Rows<'_>) -> String {
        scope
            .iter()
            .map(|crate::ty::NamedRow { binding, row }| {
                let cols = row
                    .and_then(|r| crate::ty::record_fields(db, r))
                    .map_or_else(
                        || "?".to_string(),
                        |fs| {
                            fs.iter()
                                .map(|f| f.name.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        },
                    );
                format!("{binding}{{{cols}}}")
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// **The rule `grammar.bnf` states under `SourceDef` and the checker
    /// contradicted**: *«a mapping body writes `User.name` and never
    /// `Adults.name`, even when it draws `from Adults`»*.
    ///
    /// A derived relation is not a row. It carries the rows of the bindings it
    /// derives from, under THEIR names, and its own name addresses nothing —
    /// which is both halves of the assertion below, because the second half is
    /// what stops the fix from being "accept anything".
    #[test]
    fn a_derived_relation_carries_the_binding_it_derives_from_and_not_its_own_name() {
        #[salsa::tracked]
        fn shim(db: &dyn fossil_base::Db, file: fossil_base::SourceFile) -> String {
            let scope = resolve_binding_scope(db, file, "Reachable", 0).expect("a legal pipeline");
            format!(
                "{} | Contact={} Reachable={} Nobody={}",
                render(db, &scope),
                scope.has("Contact"),
                scope.has("Reachable"),
                scope.has("Nobody"),
            )
        }

        let db = db();
        let file = fossil_base::SourceFile::new(
            &db,
            "Contact := io.csv(\"c.csv\")\nReachable := Contact.where(Contact.email != \"\")\n"
                .to_string(),
            "derived.fossil".to_string(),
        );
        assert_eq!(
            shim(&db, file),
            "Contact{?} | Contact=true Reachable=false Nobody=false",
            "`Reachable` derives from `Contact`, so `Contact` is the row a body \
             names; the relation's own name is not a row and neither is a \
             stranger's"
        );
    }

    /// **The genuine case, which must keep failing.** `Other` is a binding this
    /// file declares and this relation does not draw on, directly or derivedly.
    /// It is absent from the scope, which is the whole of what
    /// `crate::check`'s T-Column turns into the «reads a row this mapping does
    /// not have» refusal.
    #[test]
    fn a_binding_the_relation_does_not_draw_on_is_not_in_scope() {
        #[salsa::tracked]
        fn shim(db: &dyn fossil_base::Db, file: fossil_base::SourceFile) -> String {
            let scope = resolve_binding_scope(db, file, "Reachable", 0).expect("a legal pipeline");
            format!("{} | Other={}", render(db, &scope), scope.has("Other"))
        }

        let db = db();
        let file = fossil_base::SourceFile::new(
            &db,
            "Contact := io.csv(\"c.csv\")\nOther := io.csv(\"o.csv\")\n\
             Reachable := Contact.where(Contact.email != \"\")\n"
                .to_string(),
            "genuine.fossil".to_string(),
        );
        assert_eq!(
            shim(&db, file),
            "Contact{?} | Other=false",
            "a binding that exists in the file but not in this relation is still \
             a row this mapping does not have"
        );
    }

    /// **The self-join alias replaces the right side's name, and does not sit
    /// beside it.** `Node.join(Node as Other, …)` puts `Node` and `Other` in
    /// scope — two entries for one source — and `Node` on the right is not a
    /// second way to spell `Other`, because then the alias would remove no
    /// ambiguity at all.
    #[test]
    fn a_self_join_puts_both_sides_in_scope_under_two_names() {
        #[salsa::tracked]
        fn shim(db: &dyn fossil_base::Db, file: fossil_base::SourceFile) -> String {
            let scope = resolve_binding_scope(db, file, "Pairs", 0).expect("a legal pipeline");
            render(db, &scope)
        }

        let db = db();
        let file = fossil_base::SourceFile::new(
            &db,
            "Node := io.csv(\"n.csv\")\n\
             Pairs := Node.join(Node as Other, on = Node.parent == Other.id)\n"
                .to_string(),
            "selfjoin.fossil".to_string(),
        );
        assert_eq!(shim(&db, file), "Node{?} + Other{?}");
    }

    /// **The trap, measured in the one place it can bite.** Two sources with a
    /// column of the same name and a DIFFERENT TYPE. `Right.id` resolving to
    /// `Left.id`'s type would be a program that compiles and writes the other
    /// row's column, so the two sides are typed `integer` and `string` here:
    /// the assertion cannot pass by accident.
    #[test]
    fn a_qualified_reference_resolves_against_its_own_side_of_a_join() {
        #[salsa::tracked]
        fn shim(db: &dyn fossil_base::Db, file: fossil_base::SourceFile) -> String {
            let scope = resolve_binding_scope(db, file, "Both", 0).expect("a legal join");
            let ty = |binding: &str| {
                scope
                    .row_of(binding)
                    .and_then(|r| crate::ty::record_fields(db, r))
                    .and_then(|fs| fs.into_iter().find(|f| f.name == "id"))
                    .map_or_else(|| "missing".to_string(), |f| format!("{:?}", f.ty.kind(db)))
            };
            format!(
                "{} | Left.id={} Right.id={} flat={:?}",
                render(db, &scope),
                ty("Left"),
                ty("Right"),
                scope
                    .flat(db)
                    .and_then(|r| crate::ty::record_fields(db, r))
                    .map(|fs| fs.iter().map(|f| f.name.to_string()).collect::<Vec<_>>()),
            )
        }

        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::DecodingHost::default());
        let db = fossil_base::FossilDb::new(system);
        // The two `id`s differ in TYPE, which is the whole point — and that is
        // why the join is on `k` and not on them. `on = Left.id == Right.id`
        // compares an Integer with a String, and since the condition is typed
        // (`typecheck_stage`) that is a refusal, not a fixture. It was legal
        // here for as long as a stage's condition was walked for names only.
        fossil_base::test_support::register_inferred(
            &db,
            "l.csv",
            &[("id", Primitive::Integer), ("k", Primitive::String)],
        );
        fossil_base::test_support::register_inferred(
            &db,
            "r.csv",
            &[("id", Primitive::String), ("k", Primitive::String)],
        );
        let file = fossil_base::SourceFile::new(
            &db,
            "Left := io.csv(\"l.csv\")\n\
             Right := io.csv(\"r.csv\")\n\
             Both := Left.join(Right, on = Left.k == Right.k)\n"
                .to_string(),
            "join.fossil".to_string(),
        );
        assert_eq!(
            shim(&db, file),
            "Left{id,k} + Right{id,k} | Left.id=Primitive(Integer) Right.id=Primitive(String) \
             flat=Some([\"id\", \"k\", \"id\", \"k\"])",
            "each side answers with its OWN `id`; the flat row still holds both, \
             which is why a qualified reference must never go through it"
        );
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

    /// The source row an `io.rdf` member gets, one column at a time.
    ///
    /// This used to read a `ShEx` `ConstraintValue` and its three cases;
    /// it reads two fields of a decoded constraint now, and the table below is
    /// the whole of the difference. Three of the four rows produce exactly what
    /// the old `ConstraintValue` match produced:
    ///
    /// | the document says            | before   | now      |
    /// |------------------------------|----------|----------|
    /// | `xsd:integer` (in lattice)   | Integer  | Integer  |
    /// | an xsd type outside the lattice | String | String  |
    /// | `@<Shape>` (an edge)         | String   | `Iri`    |
    /// | `nodeKind IRI` (opaque IRI)  | String   | `AnyUri` |
    ///
    /// The `AnyUri` row moved because the vocabulary carries the distinction the
    /// old `ConstraintValue::Iri` threw away: an opaque IRI column is
    /// `Some(Primitive::AnyUri)` with no targets, and `AnyUri` is in the
    /// lattice. `primitive_to_graphar` writes both as `string`, so nothing
    /// downstream of the writer changes; what changes is that the checker can
    /// now tell an IRI column from a text column.
    ///
    /// The `@<Shape>` row is the one that moved next, and it had teeth: a
    /// shape-ref column is what `hasProject = .hasProject` reads, and
    /// `expected_value_ty` demands `Iri` of that predicate on the way out. While
    /// this row said `String` the copy was unsatisfiable — see
    /// [`record_from_shape`]'s own note. `subject` moved with it, being the
    /// pivoted row's entity IRI.
    #[test]
    fn a_shape_becomes_a_source_row_of_subject_plus_one_column_per_constraint() {
        use fossil_graph_schema::{Occurs, PropertyConstraint, Shape};

        let db = db();
        let constraint = |predicate: &str, datatype, targets: &[&str]| PropertyConstraint {
            predicate: predicate.to_string(),
            datatype,
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            occurs: Occurs::ONE,
            // A test fixture, not a document: no text to point into.
            span: None,
        };
        let shape = Shape {
            iri: "https://example.org/Beam".to_string(),
            properties: vec![
                constraint("https://example.org/len", Some(Primitive::Integer), &[]),
                constraint("https://example.org/note", None, &[]),
                constraint(
                    "https://example.org/in",
                    None,
                    &["https://example.org/Storey"],
                ),
                constraint("https://example.org/seeAlso", Some(Primitive::AnyUri), &[]),
            ],
        };

        let TyKind::Record(rec) = record_from_shape(&db, &shape).kind(&db) else {
            panic!("expected a Record");
        };
        let got: Vec<(&str, &TyKind<'_>)> = rec
            .fields(&db)
            .iter()
            .map(|f| (f.name.as_str(), f.ty.kind(&db)))
            .collect();
        assert_eq!(
            got,
            vec![
                // Every pivoted RDF row carries its entity IRI here, so
                // `@subject = User.subject` types — and it identifies a node of
                // THIS shape, which is what the type now says.
                (
                    "subject",
                    &TyKind::Ref(vec![SmolStr::new_static("https://example.org/Beam")]),
                ),
                ("len", &TyKind::Primitive(Primitive::Integer)),
                // The document narrowed nothing: the permissive column.
                ("note", &TyKind::Primitive(Primitive::String)),
                // An edge's source-side value is the referenced subject, and it
                // names WHICH shape — the same set `expected_value_ty` builds
                // for the same predicate on the way out, so the two agree by
                // construction rather than by both saying «an IRI».
                (
                    "in",
                    &TyKind::Ref(vec![SmolStr::new_static("https://example.org/Storey")]),
                ),
                // The one row that differs from the ShEx-typed predecessor.
                ("seeAlso", &TyKind::Primitive(Primitive::AnyUri)),
            ],
            "field names are predicate local names, in declaration order"
        );
    }
}
