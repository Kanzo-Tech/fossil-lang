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
//!    the `duckdb` crate), `resolve_source_row` consumes that descriptor and
//!    builds the [`Record`] directly — no CSVW JSON is read from disk.
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
//! There was a step between 1 and 2, and it is gone: a CSVW descriptor named by
//! `schema = "<path>"`, read through `System::read_file` under a
//! `D-CSVW-DEPRECATED` diagnostic whose own text told the author to delete the
//! argument because «types will be inferred from the file directly». It is
//! deleted rather than modelled — giving CSVW a row in the provider registry
//! would have resurrected a deprecated feature so the new model could express
//! it, and the model leaving it nowhere to sit is the model agreeing with the
//! deprecation.
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
use crate::ty::{Record, RecordField, Ty, TyKind};

/// The source-row [`Ty`] (a `Record`) for a mapping as known from the
/// host-registered [`fossil_descriptors_input::InferredDescriptor`] ONLY.
///
/// Side-effect-free: no diagnostics, no filesystem reads — the
/// descriptor-branch of [`resolve_source_row`] without the type-check's
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
    let hir_mapping = mappings.mappings(db).get(mapping.index(db))?;
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

/// The rows a relation makes addressable, each under the name of the BINDING
/// that introduced it.
///
/// This is the consequence of names `grammar.bnf` spells out under
/// `SourceDef`: *«a mapping body writes `User.name` and never
/// `Adults.name`, even when it draws `from Adults`»*. A binding ties the type
/// and the relation together, so a relation DERIVED from `User` — by `where`, by
/// `select`, by standing on the left of a `join` — keeps handing back rows that
/// are addressed as `User`. The derived name (`Adults`, `Reachable`, `Joined`)
/// names the relation and never a row.
///
/// Which is why this is a LIST and not one record. A join brings a second
/// binding into the same relation, and its columns stay under their own name:
/// `Purchase.amount` and `User.email` are two rows of one relation, and two
/// columns called `id` — one per side — are two distinct entries here even
/// though the flattened [`Self::flat`] record can only find the first. Ruling 17
/// of `SURFACE-PLAN.md` deleted the collision rule on the promise that
/// qualification would do that work; this is where it does it.
///
/// The row is an `Option` because a binding whose source declares no schema is
/// still a row a body may name: `hello.fossil` addresses columns of a source
/// with no descriptor at all. So membership (does this relation have a row
/// called `Contact`?) and typing (what is `Contact.email`?) are two different
/// questions, and only the first has an answer for every program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowScope<'db> {
    rows: Vec<(SmolStr, Option<Ty<'db>>)>,
}

impl<'db> RowScope<'db> {
    /// The scope of a binding that reads a file: itself, and nothing else.
    #[must_use]
    pub fn one(binding: &str, row: Option<Ty<'db>>) -> Self {
        Self {
            rows: vec![(SmolStr::from(binding), row)],
        }
    }

    /// The binding names this relation makes addressable, left to right.
    pub fn bindings(&self) -> impl Iterator<Item = &SmolStr> {
        self.rows.iter().map(|(name, _)| name)
    }

    /// Is there a row under this name — the question the qualified-reference
    /// diagnostic asks. TRUE with an untyped row; absence is not "no schema".
    #[must_use]
    pub fn has(&self, binding: &str) -> bool {
        self.rows.iter().any(|(name, _)| name == binding)
    }

    /// The row a binding contributes. `None` both when the name is not in scope
    /// and when it is but its source declares no schema — ask [`Self::has`]
    /// first, because those two are different answers.
    #[must_use]
    pub fn row_of(&self, binding: &str) -> Option<Ty<'db>> {
        self.rows
            .iter()
            .find(|(name, _)| name == binding)
            .and_then(|(_, row)| *row)
    }

    /// Every column of every row, left to right, as one flat `Record` — what
    /// the checker resolves a BARE name against and what the row algebra prints
    /// in its refusals.
    ///
    /// `None` when any row in the scope is untyped: a record missing one side's
    /// columns would answer "unknown column" for a column that exists.
    #[must_use]
    pub fn flat(&self, db: &'db dyn fossil_base::Db) -> Option<Ty<'db>> {
        let fields = self.fields(db)?;
        Some(Ty::new(db, TyKind::Record(Record::new(db, fields))))
    }

    /// [`Self::flat`]'s fields, before they are interned.
    fn fields(&self, db: &'db dyn fossil_base::Db) -> Option<Vec<RecordField<'db>>> {
        let mut out = Vec::new();
        for (_, row) in &self.rows {
            out.extend(record_fields(db, (*row)?)?);
        }
        Some(out)
    }

    /// The columns of ONE row, for a per-binding message.
    fn fields_of(
        &self,
        db: &'db dyn fossil_base::Db,
        binding: &str,
    ) -> Option<Vec<RecordField<'db>>> {
        record_fields(db, self.row_of(binding)?)
    }

    /// Both sides of a join, in written order.
    fn concat(mut self, other: Self) -> Self {
        self.rows.extend(other.rows);
        self
    }

    /// `Node as Other` — the right side of a self-join under its second name.
    ///
    /// The whole right scope collapses to one row, because the alias is one
    /// name: joining a multi-binding relation under an alias makes its columns
    /// reachable through the alias and through nothing else.
    fn rename_to(self, db: &'db dyn fossil_base::Db, alias: &SmolStr) -> Self {
        let row = self.flat(db);
        Self {
            rows: vec![(alias.clone(), row)],
        }
    }
}

/// The scope a mapping's `from` clause puts in the body — see [`RowScope`].
///
/// Plain-Rust helper (NOT `#[salsa::tracked]`) — called from within the
/// `typecheck_mapping` tracked query so its `delay_span_bug` emits are valid.
///
/// Reads `def_map(db, file)` only (NEVER walks the FILE CST from
/// `mapping_cst_node`) — see the module docs for the Serious #6 rationale.
pub fn resolve_source_scope<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Result<Option<RowScope<'db>>, fossil_base::ErrorGuaranteed> {
    let file = mapping.file(db);

    // 1. Find the mapping's source binding name.
    let mappings = crate::lower::lower_to_hir(db, file);
    let Some(hir_mapping) = mappings.mappings(db).get(mapping.index(db)) else {
        return Ok(None);
    };
    let source_name = hir_mapping.source_binding.clone();

    resolve_binding_scope(db, file, source_name.as_str(), 0).map(Some)
}

/// The source-row [`Ty`] a mapping's body resolves a BARE name against — the
/// flattening of [`resolve_source_scope`].
///
/// A qualified reference must not come through here: flattening is what loses
/// the binding, and after a join it is what makes two columns called `id`
/// answer as one.
pub fn resolve_source_row<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Result<Option<Ty<'db>>, fossil_base::ErrorGuaranteed> {
    Ok(resolve_source_scope(db, mapping)?.and_then(|scope| scope.flat(db)))
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

/// The [`RowScope`] of a source BINDING, by name.
///
/// Split out of [`resolve_source_scope`] because a pipeline's row is its base's
/// row transformed, and the base is a binding, not a mapping. Everything below
/// the pipeline branch is what the function has always done for a binding that
/// reads a file — and it is where the scope BOTTOMS OUT at one row under one
/// name, which is why a derived relation can never invent a binding: it can only
/// carry, restrict or extend the names its base already had.
pub fn resolve_binding_scope<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    source_name: &str,
    depth: usize,
) -> Result<RowScope<'db>, fossil_base::ErrorGuaranteed> {
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
    Ok(RowScope::one(
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
            ShapeBindError::Arity { declared, named } => format!(
                "the binding names {named} shape(s) and the document declares {declared}; \
                 names bind by position, so `{source_name}` has no shape to bind"
            ),
        };
        let _eg = delay_span_bug(db, Span::new(0, 0), message);
        return Ok(None);
    }

    // There is no fallback below this, and that is the change. A `schema = "…"`
    // holding a CSVW descriptor used to build the row here, under a
    // `D-CSVW-DEPRECATED` diagnostic that told the author to delete the
    // argument because «types will be inferred from the file directly». Adding
    // a row for CSVW to the provider registry would have resurrected a
    // deprecated feature so the new model could express it; the model leaving
    // it nowhere to sit is the model agreeing with the deprecation.
    //
    // So a source with no INFERRED descriptor has no row, and that is not an
    // error — it is every schemaless program in the tree. The host introspects
    // (`fossil_engine::pre_introspect_and_register`, the browser's
    // `registerInferredDescriptor`), and what it finds arrives above.
    Ok(None)
}

/// One verb of a source pipeline applied to the [`RowScope`] it receives.
///
/// Every refusal names the pipeline and the columns it actually has: a row
/// algebra whose errors say "column not found" and stop is a row algebra nobody
/// can debug from the message.
///
/// It took a `Ty` and gave one back. The scope is what a `join` needs to hand
/// on: two sides that stay addressable under their own binding names is exactly
/// what ruling 17 promised when it deleted the collision rule, and a flat
/// `Record` cannot carry it.
fn apply_source_op<'db>(
    db: &'db dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    pipe: &crate::lower::HirSourcePipe,
    op: &crate::lower::HirSourceOp,
    scope: RowScope<'db>,
    depth: usize,
) -> Result<RowScope<'db>, fossil_base::ErrorGuaranteed> {
    use crate::lower::HirSourceOp;

    // An untyped input is not an error — it is every schemaless program in the
    // tree — and nothing below can check a column against a row nobody
    // declared. The NAMES still flow, so the scope is returned rather than
    // dropped.
    let Some(fields) = scope.fields(db) else {
        return match op {
            // A join still has to bring the right-hand names in, or a body that
            // writes `User.email` next to an untyped `Purchase` would be told
            // `User` is not a row this mapping has — which is false, and is the
            // diagnostic this whole seam exists to stop being wrong.
            HirSourceOp::Join { right, alias, .. } => {
                Ok(scope.concat(right_scope(db, file, right, alias.as_ref(), depth)?))
            }
            HirSourceOp::Where(_) | HirSourceOp::Select(_) => Ok(scope),
        };
    };
    match op {
        // `where` keeps rows, not columns: the row type is its input's. The
        // columns the predicate names still have to exist — a filter on a column
        // that is not there is a program that would run and keep everything.
        HirSourceOp::Where(pred) => {
            check_refs(db, pipe, "where", pred, &scope, &fields)?;
            Ok(scope)
        }
        // `select` restricts, and it restricts EACH ROW: `Employee.id` stays a
        // column of `Employee` after `Active.select(Employee.id, …)`, because
        // the binding is what a body writes. The payload has lost its
        // qualification (`HirSourceOp::Select` carries column names only), so a
        // name is looked for in the rows in order and taken from the first that
        // has it — which is the one place a qualified reference still cannot be
        // told apart from a bare one, and `select` over a join is left open for
        // that reason.
        HirSourceOp::Select(cols) => {
            let mut kept: Vec<(SmolStr, Vec<RecordField<'db>>)> =
                scope.bindings().map(|b| (b.clone(), Vec::new())).collect();
            for col in cols {
                let mut found = false;
                for (binding, out) in &mut kept {
                    if let Some(f) = scope
                        .fields_of(db, binding)
                        .and_then(|fs| fs.into_iter().find(|f| &f.name == col))
                    {
                        out.push(f);
                        found = true;
                        break;
                    }
                }
                if !found {
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
                }
            }
            Ok(RowScope {
                rows: kept
                    .into_iter()
                    .map(|(binding, fs)| {
                        (
                            binding,
                            Some(Ty::new(db, TyKind::Record(Record::new(db, fs)))),
                        )
                    })
                    .collect(),
            })
        }
        // The join. **The flattening died and so did the collision rule**
        // (ruling 17 of `SURFACE-PLAN.md`).
        //
        // This used to compute `fila(izq) ⊎ fila(der)` — one FLAT record — and
        // treat any shared name other than the key as an error whose message was
        // "rename one side before joining". That rule existed to remove an
        // ambiguity that no longer exists: the body writes
        // `Purchase.amount` and `User.email`, so both sides stay addressable
        // under their binding's name and a shared column name means nothing.
        // The rule is therefore DELETED, not relaxed — qualification is exactly
        // what it was standing in for.
        //
        // **This is the one change in this phase that alters the meaning of a
        // program that is accepted today**: two sources with a column of the
        // same name go from rejected to legal.
        //
        // The seam this note used to leave open — «the result is still a flat
        // `Record`, so it can now HOLD two fields of the same name but a lookup
        // by bare name still finds the first» — is closed by the scope: the two
        // sides are two entries, `Purchase.id` and `User.id` land on different
        // ones, and only a BARE name (which the surface no longer has a spelling
        // for) still flattens into finding the first.
        HirSourceOp::Join { right, alias, on } => {
            let right = right_scope(db, file, right, alias.as_ref(), depth)?;
            let Some(right_fields) = right.fields(db) else {
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
            let joined = scope.concat(right);
            let mut all = fields;
            all.extend(right_fields);
            check_refs(db, pipe, "the `on` condition of `join`", on, &joined, &all)?;
            Ok(joined)
        }
    }
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
) -> Result<RowScope<'db>, fossil_base::ErrorGuaranteed> {
    let scope = resolve_binding_scope(db, file, right.as_str(), depth + 1)?;
    Ok(match alias {
        Some(a) => scope.rename_to(db, a),
        None => scope,
    })
}

/// Every reference a verb's expression makes, checked against the scope that
/// verb receives — the row-algebra half of the rule
/// [`crate::check::Checker::synth_ty`] applies to a mapping body.
///
/// One function for `where` and for `join`'s `on` because "the columns this
/// predicate names must exist" is one rule, and it has three ways to fail: a
/// binding the relation does not carry, a column that binding does not have,
/// and a bare name nothing has.
fn check_refs<'db>(
    db: &'db dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    verb: &str,
    expr: &crate::lower::HirExpr,
    scope: &RowScope<'db>,
    flat: &[RecordField<'db>],
) -> Result<(), fossil_base::ErrorGuaranteed> {
    let mut named = Vec::new();
    collect_refs(expr, &mut named);
    for reference in named {
        match reference {
            Reference::Qualified { binding, column } => {
                let Some(row) = scope.fields_of(db, binding.as_str()) else {
                    return Err(pipe_error(
                        db,
                        pipe,
                        format!(
                            "`{verb}` in `{}` reads `{binding}.{column}`, and `{}` carries no row \
                             called `{binding}`. It draws on: {}",
                            pipe.name,
                            pipe.name,
                            binding_list(scope),
                        ),
                    ));
                };
                if !row.iter().any(|f| f.name == column) {
                    return Err(pipe_error(
                        db,
                        pipe,
                        format!(
                            "`{verb}` in `{}` reads `{binding}.{column}`, which `{binding}` does \
                             not have. It has: {}",
                            pipe.name,
                            column_list(&row),
                        ),
                    ));
                }
            }
            Reference::Bare(column) => {
                if !flat.iter().any(|f| f.name == column) {
                    return Err(pipe_error(
                        db,
                        pipe,
                        format!(
                            "`{verb}` in `{}` reads `.{column}`, which `{}` does not have. \
                             It has: {}",
                            pipe.name,
                            pipe.base,
                            column_list(flat),
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The binding names a relation draws on, for a message that has to say which
/// rows the author could have written instead.
fn binding_list(scope: &RowScope<'_>) -> String {
    let names: Vec<String> = scope.bindings().map(|b| format!("`{b}`")).collect();
    if names.is_empty() {
        "no rows at all".to_string()
    } else {
        names.join(", ")
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

/// A reference an expression makes to a column of a row.
///
/// Two variants because the surface has one spelling and the HIR still has two:
/// `HirExpr::ColumnRef` is what `grammar.bnf` writes today, and
/// `HirExpr::FieldRef` is the anonymous form it replaced. They are checked
/// differently — a qualified reference names its row and can be wrong about it;
/// a bare one can only be wrong about the column.
enum Reference {
    Qualified { binding: SmolStr, column: SmolStr },
    Bare(SmolStr),
}

/// Every column an expression names, in order, duplicates included.
fn collect_refs(expr: &crate::lower::HirExpr, out: &mut Vec<Reference>) {
    use crate::lower::HirExpr;
    match expr {
        HirExpr::FieldRef(name) => out.push(Reference::Bare(name.clone())),
        HirExpr::ColumnRef { binding, column } => out.push(Reference::Qualified {
            binding: binding.clone(),
            column: column.clone(),
        }),
        HirExpr::BinOp { lhs, rhs, .. } => {
            collect_refs(lhs, out);
            collect_refs(rhs, out);
        }
        HirExpr::UnaryOp { operand, .. } => collect_refs(operand, out),
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => {
            collect_refs(cond, out);
            collect_refs(then, out);
            collect_refs(otherwise, out);
        }
        // An edge constructor reads columns through its ARGUMENTS and through
        // nothing else. The target type's template is written in the DEFINING
        // mapping's scope and its holes are replaced wholesale, so none of its
        // column references belongs to the row this walker is checking — see
        // `identity::SubjectTemplate::fill`.
        HirExpr::Call { args, .. } | HirExpr::Edge { args, .. } => {
            for a in args {
                collect_refs(a, out);
            }
        }
        // A subject IRI reads columns, and until the holes were parsed this
        // walker could not see a single one of them.
        HirExpr::Interpolation(parts) => {
            for p in parts {
                if let crate::lower::InterpolationPart::Hole(e) = p {
                    collect_refs(e, out);
                }
            }
        }
        HirExpr::StringLit(_) | HirExpr::IntLit(_) | HirExpr::FloatLit(_) | HirExpr::BoolLit(_) => {
        }
    }
}

fn pipe_error(
    db: &dyn fossil_base::Db,
    pipe: &crate::lower::HirSourcePipe,
    message: String,
) -> fossil_base::ErrorGuaranteed {
    delay_span_bug(db, Span::new(pipe.span.0, pipe.span.1), message)
}

/// Build a `Record` [`Ty`] from a decoded [`Shape`] — the COMPILE-TIME source
/// row type for an `io.rdf` destructuring member. One field per constraint plus
/// the always-present
/// `subject` IRI column (the pivoted RDF row carries the entity IRI there, so
/// `iri = .subject` types).
///
/// Two rules, and they are the same two the output decomposition applies
/// (`fossil-sinks::decomp::classify_object`):
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
    let iri_ty = Ty::new(db, TyKind::Iri);
    // Every pivoted RDF row carries its entity IRI in `subject`.
    let mut fields: Vec<RecordField<'db>> = vec![RecordField {
        name: SmolStr::new_static("subject"),
        ty: iri_ty,
    }];
    for c in &shape.properties {
        let ty = if c.targets.is_empty() {
            Ty::new(
                db,
                TyKind::Primitive(c.datatype.unwrap_or(Primitive::String)),
            )
        } else {
            iri_ty
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
/// The CSVW-descriptor twin this was written to mirror is gone, and the
/// semantic-equivalence invariant (INPUT-03) that the two build the same
/// `Record` from the same columns went with it: there is one path now.
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    /// A one-line rendering of a scope: `Contact{id,email} + Other{id}`.
    ///
    /// The binding NAMES are what these tests are about, so they are in the
    /// string and not behind an accessor call per assertion.
    fn render(db: &dyn fossil_base::Db, scope: &RowScope<'_>) -> String {
        scope
            .rows
            .iter()
            .map(|(binding, row)| {
                let cols = row.and_then(|r| record_fields(db, r)).map_or_else(
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
    /// column of the same name and a DIFFERENT TYPE: ruling 17 made that legal
    /// and left the seam open, because the joined row was one flat `Record` and
    /// a lookup by name found the first entry.
    ///
    /// So `Right.id` resolving to `Left.id`'s type would be the failure that is
    /// worse than the bug this whole change fixes — a program that compiles and
    /// writes the other row's column. The two sides are typed `integer` and
    /// `string` here for exactly that reason: the assertion cannot pass by
    /// accident.
    #[test]
    fn a_qualified_reference_resolves_against_its_own_side_of_a_join() {
        #[salsa::tracked]
        fn shim(db: &dyn fossil_base::Db, file: fossil_base::SourceFile) -> String {
            let scope = resolve_binding_scope(db, file, "Both", 0).expect("a legal join");
            let ty = |binding: &str| {
                scope
                    .row_of(binding)
                    .and_then(|r| record_fields(db, r))
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
                    .and_then(|r| record_fields(db, r))
                    .map(|fs| fs.iter().map(|f| f.name.to_string()).collect::<Vec<_>>()),
            )
        }

        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::DecodingHost::default());
        let db = fossil_base::FossilDb::new(system);
        fossil_base::test_support::register_inferred(&db, "l.csv", &[("id", Primitive::Integer)]);
        fossil_base::test_support::register_inferred(&db, "r.csv", &[("id", Primitive::String)]);
        let file = fossil_base::SourceFile::new(
            &db,
            "Left := io.csv(\"l.csv\")\n\
             Right := io.csv(\"r.csv\")\n\
             Both := Left.join(Right, on = Left.id == Right.id)\n"
                .to_string(),
            "join.fossil".to_string(),
        );
        assert_eq!(
            shim(&db, file),
            "Left{id} + Right{id} | Left.id=Primitive(Integer) Right.id=Primitive(String) \
             flat=Some([\"id\", \"id\"])",
            "each side answers with its OWN `id`; the flat row still holds both, \
             which is why a qualified reference must never go through it"
        );
    }

    // `USERS_CSVW`, `record_from_descriptor_maps_columns_to_primitives` and
    // `record_from_inferred_matches_csvw_path_on_same_shape` lived here. The
    // last of the three asserted that the CSVW path and the inferred path build
    // the same `Record` from the same columns — a real invariant while there
    // were two paths, and a statement about nothing now that there is one.

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
                // `@subject = User.subject` types.
                ("subject", &TyKind::Iri),
                ("len", &TyKind::Primitive(Primitive::Integer)),
                // The document narrowed nothing: the permissive column.
                ("note", &TyKind::Primitive(Primitive::String)),
                // An edge's source-side value is the referenced subject's IRI,
                // and now says so — `expected_value_ty` demands `Iri` of the
                // same predicate on the way out.
                ("in", &TyKind::Iri),
                // The one row that differs from the ShEx-typed predecessor.
                ("seeAlso", &TyKind::Primitive(Primitive::AnyUri)),
            ],
            "field names are predicate local names, in declaration order"
        );
    }
}
