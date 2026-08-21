//! [`DefMap`] — per-file name resolution table.
//!
//! Holds the source bindings (`User := io.csv("...")`), the TYPE bindings
//! (`type { Person } := io.shex("...")`), and the mapping list (one entry per
//! `Name : Shape from source` block). Populated by the [`def_map`] Salsa query,
//! which consumes the CST from [`fossil_syntax::parse`].
//!
//! # There was a prefix table here, and its absence is the point
//!
//! `prefixes: Vec<PrefixEntry>` was the first field of [`DefMap`], and a
//! `&[PrefixEntry]` slice was threaded through nine functions in
//! [`crate::lower`] to expand `ex:Person` into `https://example.org/Person`. The
//! CURIE is gone from every position it held — a shape name, a property key, an
//! expression and an interpolation hole are all bare names or full IRIs in
//! strings now — so there is nothing left to expand.
//!
//! What replaces it for a mapping's SHAPE is [`DefMap::lookup_type`], which was
//! already here: the header names one of the bare names a `type { … } := …`
//! binding introduced, and that binding already knew the shape IRI. The
//! resolution moved from a text substitution the program spelled out to a
//! lookup against a document the program names — and naming one is MANDATORY,
//! because a bare name has nowhere else to come from. It is why a shape name can
//! now be MISSPELT and reported rather than expanded into an IRI that happens to
//! denote nothing.
//!
//! Mapping and source
//! locations are interned with a `'db` lifetime so downstream queries can be
//! keyed per-item (rust-analyzer's per-item Salsa fan-out). Even though the
//! Phase 1 example has only one mapping, the SHAPE of the query graph matters
//! for Phase 2's stable item tree (CORE-02) and Phase 6's LSP perf benchmark.
//!
//! # Storage choice
//!
//! The source and type tables are insertion-ordered `Vec`s rather than the
//! originally-planned `IndexMap<K, V>`. The workspace
//! `indexmap = "2"` pin does not implement [`std::hash::Hash`] on `IndexMap`,
//! and Salsa tracked-struct fields require Hash for memoisation. Insertion
//! order is preserved either way; callers that need O(1) lookup can build a
//! `HashMap` at the call site (or use the [`DefMap::lookup_source`] helper
//! added below).

use fossil_base::{SourceFile, Span};
use smol_str::SmolStr;

#[salsa::interned(debug)]
pub struct MappingLoc<'db> {
    pub file: SourceFile,
    pub index: usize,
}

#[salsa::interned(debug)]
pub struct SourceLoc<'db> {
    pub file: SourceFile,
    pub index: usize,
}

/// Why a destructuring member bound no shape.
///
/// Four causes used to collapse into one `None`, and the single diagnostic that
/// existed blamed the member's NAME for all of them — so an unreadable file
/// reported "matches no shape in the schema". They are separated here because
/// the reader cannot act on a message that names the wrong thing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ShapeBindError {
    /// The constructor carries no `schema = …`, so there is no document.
    NoSchema,
    /// The document is named and is not there — nothing is registered at the
    /// path the program wrote.
    Unreadable { path: SmolStr, cause: SmolStr },
    /// The document is there and cannot be read as a shape document: no
    /// installed decoder claims it, or one ran and rejected it.
    Unparseable { path: SmolStr, cause: SmolStr },
    /// The binding names more shapes than the document declares. Binding is
    /// POSITIONAL, so this is the check that model gives
    /// away free: the Nth name wants an Nth shape and there is none.
    Arity { declared: usize, named: usize },
}

/// One entry in the source-binding table (`users := io.csv(...)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceEntry<'db> {
    pub name: SmolStr,
    pub loc: SourceLoc<'db>,
    /// The DOCUMENT named by the `schema =` argument — the path inside the
    /// provider call: `{ A, B } := io.rdf("g.ttl", schema = io.shex("x.shex"))`
    /// gives `x.shex`.
    ///
    /// [`crate::infer::resolve_source_scope`] reads it to type the source's rows
    /// forward. It is a SIGNATURE-only datum:
    /// it is read from the `SOURCE_DEF` header tokens, NOT from any mapping
    /// body, so it does NOT widen the per-mapping `body()` fan-out. The
    /// `def_map` query is file-keyed and structurally stable across
    /// body-only edits (verified by `tests/invalidation_regression.rs`).
    pub schema_arg: Option<SmolStr>,
    /// The PROVIDER that argument names — `io.shex`, `io.shacl`.
    ///
    /// `schema = "x.shex"` was a bare path, and it was the last position in the
    /// language where a document was named without a row to read it: the
    /// document got its decoder from its own extension. `schema =` takes a
    /// provider call now, so there is one rule — where there is a document,
    /// there is a row that names it — and `None` here means the program wrote a
    /// bare string, which `crate::lower::check_provider` reports with a span.
    pub schema_provider: Option<SmolStr>,
    /// For a destructuring RDF source member (`{ IfcBeam, ... } := io.rdf(...,
    /// schema = io.shex("x.shex"))`), the shape IRI this member's local-name resolves to
    /// in the schema (`IfcBeam` ↔ `http://ifcowl.../IfcBeam`), resolved at
    /// COMPILE TIME against the document. Subject selection is always by `rdf:type ==
    /// shape_iri`. `None` for plain single-binding native sources (csv/json/…).
    pub shape_iri: Option<SmolStr>,
    /// Set iff this is a destructuring member and [`Self::shape_iri`] is `None`
    /// — why it is `None`. `None` here for every non-member binding.
    pub shape_error: Option<ShapeBindError>,
    /// Dotted name of the source constructor (`io.csv` / `io.json` /
    /// `io.parquet`), if a call-shaped RHS could be parsed. The
    /// constructor name selects the source FORMAT downstream
    /// (`fossil-mir::lower` maps it to `SourceFormat`). Like [`Self::schema_arg`]
    /// this is a SIGNATURE-only `SOURCE_DEF`-header datum (Phase 5 STDL-06).
    pub constructor: Option<SmolStr>,
    /// The first POSITIONAL string argument of the constructor — the source
    /// URI (`io.csv("examples/users.csv")` → `examples/users.csv`). Resolved by
    /// `fossil-mir::lower` into `Op::Source.uri`, replacing the Phase-1
    /// hardcoded `examples/users.csv`. Signature-only (Phase 5 STDL-06).
    pub uri: Option<SmolStr>,
    /// The whole `User := io.csv("users.csv")` item, FILE-ABSOLUTE — where to
    /// point when a diagnostic is about the row this binding introduced rather
    /// than about the line that read it.
    ///
    /// `User.nmae` is blamed at the property, and the second half of the
    /// sentence — which fields `User` actually has — belongs at the binding
    /// that answers it. That is a label with its own `SpanFrame`
    /// ([`fossil_base::SpanLabel`]), riding inside a mapping-relative
    /// diagnostic.
    ///
    /// **It is recorded HERE, and not looked up when the diagnostic is
    /// emitted.** A per-mapping query that reached for `parse(db, file)` to
    /// find it would tie every mapping's type-check to the whole-file CST and
    /// break the fan-out barrier `crate::body::mapping_cst_node` exists to
    /// keep (`tests/invalidation_regression.rs`). `def_map` is file-keyed,
    /// already walking these nodes, and structurally stable across body-only
    /// edits — the same argument [`Self::schema_arg`] makes.
    pub span: Span,
}

/// One name bound by a type binding (`type { Person, City } := io.shex("s.shex")`).
///
/// The value side of the language binds sources; this binds TYPES. One
/// catalogue (`io.*`) and ONE binder: `:=` binds a NAME, and the left-hand side
/// says what kind of name it is (grammar.bnf, DEFINE). Types had their own `=`
/// for a while and lost it: `=` assigns a value, and a type binding assigns
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeEntry {
    /// The local label. Free — it does not have to name anything in the
    /// document, because binding is by position — the Nth name takes the Nth
    /// declared shape, whatever it is called. This is what lets two documents
    /// that both declare `Person` coexist.
    pub name: SmolStr,
    /// The shape IRI this name binds: the Nth declared shape for the Nth name.
    pub shape_iri: Option<SmolStr>,
    /// Why it bound nothing, when it bound nothing. Never a bare `None`.
    pub shape_error: Option<ShapeBindError>,
    /// The document the constructor names — `io.shex("personas.shex")`. This is
    /// the datum that gives a CSV-sourced program somewhere to declare its
    /// output shape: a program whose source is not RDF names a shape document
    /// nowhere else, and without one it has no output contract at all.
    pub document: Option<SmolStr>,
    /// The constructor — `io.shex`, `io.shacl`. It was parsed and thrown away
    /// (`let (_ctor, document) = …`) and every dispatch went by the document's
    /// EXTENSION, which is why `io.shex("x.ttl")` and `io.shacl("x.ttl")` were
    /// the same program. Ruling 13: the NAME selects the registry row.
    pub constructor: Option<SmolStr>,
    /// `@rename(Person, "http://xmlns.com/foaf/0.1/name" as foaf_name)` — the
    /// renames written above this binding and addressed to THIS name, as
    /// `(predicate IRI, the name to write instead)` in source order.
    ///
    /// Empty for every binding that carries none, which is almost all of them:
    /// a rename is the repair for two predicates whose last IRI segments
    /// coincide, and the repair is the exception. Property keys are bare names
    /// on the bet that such a collision is rare; a vocabulary where it is the
    /// norm is what would reverse that and bring qualification back.
    ///
    /// Addressed to a NAME and not to the file: one `type { … }` line binds
    /// several names, `@rename`'s first argument says which, and two shapes in
    /// one document can each have a colliding `name` needing different repairs.
    /// [`crate::shapes::ResolvedShape::short_names`] applies them.
    pub renames: Vec<(SmolStr, SmolStr)>,
}

#[salsa::tracked(debug)]
pub struct DefMap<'db> {
    #[returns(ref)]
    pub sources: Vec<SourceEntry<'db>>,
    #[returns(ref)]
    pub mappings: Vec<MappingLoc<'db>>,
    #[returns(ref)]
    pub types: Vec<TypeEntry>,
}

// `DefMap::lookup_prefix` lived here and had exactly one caller: its own unit
// test. The free function of the same name in `crate::lower` had four
// (`lower.rs` 568, 944, 1024, 1055) and WAS the language — and this tombstone
// already said it was going with `prefix` at step 3. It has: both are gone, and
// so is the `&[PrefixEntry]` slice the four threaded between them.

impl<'db> DefMap<'db> {
    /// Look up the [`SourceLoc`] bound to a source name (e.g. `users`).
    #[must_use]
    pub fn lookup_source(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SourceLoc<'db>> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.loc)
    }

    /// Where a source name was bound — [`SourceEntry::span`], file-absolute.
    #[must_use]
    pub fn lookup_source_span(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<Span> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.span)
    }

    /// The document named by the `schema =` argument of a source, if it named
    /// one. `fossil-lineage` reports it as a reference and the engine registers
    /// it; the compiler wants the pair below.
    #[must_use]
    pub fn lookup_source_schema(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SmolStr> {
        self.lookup_source_schema_binding(db, name)
            .map(|(_, document)| document)
    }

    /// The `(provider, document)` pair that argument names —
    /// `schema = io.shex("x.shex")`.
    ///
    /// The pair, for the reason [`Self::output_shape_binding`] is a pair: the
    /// provider is what selects the row that reads the document, and handing
    /// them out separately is how a caller comes to read one and forget the
    /// other. That is not hypothetical here — `crate::infer` did exactly that
    /// and passed `None` for the provider, which turned every destructuring RDF
    /// source into "the document is named by no provider".
    #[must_use]
    pub fn lookup_source_schema_binding(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<(Option<SmolStr>, SmolStr)> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.schema_arg.clone().map(|d| (e.schema_provider.clone(), d)))
    }

    /// Look up the resolved shape IRI bound to a destructuring RDF source member
    /// (`{ IfcBeam, ... } := io.rdf(..., schema = io.shex("x.shex"))` → `IfcBeam`'s shape
    /// IRI). `None` for native single-binding sources. Used by the input typing
    /// ([`crate::infer::resolve_source_scope`]) to resolve the member's
    /// compile-time row type from the declared shape.
    #[must_use]
    pub fn lookup_source_shape_iri(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<SmolStr> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_iri.clone())
    }

    /// Why a destructuring member bound no shape. `None` when it bound one, or
    /// when the binding never named a shape at all.
    #[must_use]
    pub fn lookup_source_shape_error(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<ShapeBindError> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_error.clone())
    }

    /// Look up the `(constructor, uri)` pair bound to a source name (e.g.
    /// `users` → `("io.csv", "examples/users.csv")`). Used by Phase 5
    /// `fossil-mir::lower` (STDL-06) to resolve `Op::Source`'s format + URI
    /// from the real binding instead of the Phase-1 hardcode. Either component
    /// is `None` when the `SOURCE_DEF` RHS is not a recognisable
    /// `io.*("...")` call.
    #[must_use]
    pub fn lookup_source_call(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<(Option<SmolStr>, Option<SmolStr>)> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| (e.constructor.clone(), e.uri.clone()))
    }

    /// The shape IRI a `type { … } := io.shex(…)` name binds.
    #[must_use]
    pub fn lookup_type(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SmolStr> {
        self.types(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_iri.clone())
    }

    /// The `@rename`s addressed to the binding that introduced `shape_iri`, as
    /// `(predicate IRI, the name to write instead)`.
    ///
    /// Keyed by the SHAPE and not by the local name, because that is what the
    /// caller has: a mapping header names a type, `lower_to_hir` resolves it to
    /// a shape IRI, and by the time [`crate::check`] has a `ResolvedShape` the
    /// local name is behind it. Empty when the file has no rename for that
    /// shape, which is the ordinary case.
    #[must_use]
    pub fn renames_for_shape(
        self,
        db: &'db dyn fossil_base::Db,
        shape_iri: &str,
    ) -> Vec<(SmolStr, SmolStr)> {
        self.types(db)
            .iter()
            .find(|e| e.shape_iri.as_deref() == Some(shape_iri))
            .map_or_else(Vec::new, |e| e.renames.clone())
    }

    /// Every `@rename` the file wrote, addressed by shape — what the WRITER
    /// needs, where [`Self::renames_for_shape`] is what the checker needs.
    ///
    /// The two halves used to be one: the checker read the table and
    /// `OutputShapes::to_graph_schema` did not, so a repaired collision
    /// type-checked under its new name and shipped under the old one. This is
    /// the whole table because a document declares several shapes and the run
    /// lowers all of them at once.
    #[must_use]
    pub fn renames(self, db: &'db dyn fossil_base::Db) -> fossil_graph_schema::Renames {
        fossil_graph_schema::Renames::new(
            self.types(db)
                .iter()
                .filter(|e| !e.renames.is_empty())
                .filter_map(|e| {
                    e.shape_iri.as_ref().map(|iri| {
                        (
                            iri.to_string(),
                            e.renames
                                .iter()
                                .map(|(p, n)| (p.to_string(), n.to_string()))
                                .collect(),
                        )
                    })
                })
                .collect(),
        )
    }

    /// Why a type name bound nothing. `None` when it bound a shape.
    #[must_use]
    pub fn lookup_type_error(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<ShapeBindError> {
        self.types(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_error.clone())
    }

    /// The first shape document the file brings in, if any.
    ///
    /// This is what a mapping's OUTPUT shape is resolved against when the source
    /// is a CSV — the case that had nowhere to declare a shape at all until a
    /// `type` binding gave it one. One document per file for now: a second
    /// `type … =` is legal and binds its own names, but which document backs the
    /// output contract is not something any program has had to say yet.
    #[must_use]
    pub fn output_shape_document(self, db: &'db dyn fossil_base::Db) -> Option<SmolStr> {
        self.output_shape_binding(db).map(|(_, document)| document)
    }

    /// The `(constructor, document)` pair of that same first binding — what
    /// ruling 13 needs, because the constructor is what selects the registry row
    /// and the document is what it is asked to read. They come back together so
    /// no caller can read one and forget the other, which is exactly how they
    /// drifted apart.
    ///
    /// **This is the PROGRAM's output descriptor, not a mapping's contract.**
    /// The run writes one corpus and classifies its edges against one schema
    /// (`fossil_engine::resolve_output_descriptor`, v1). A mapping's contract is
    /// resolved by TYPE — [`Self::shape_binding_for`] — because a program may
    /// bring in two documents and a mapping is checked against the one that
    /// declared ITS type.
    #[must_use]
    pub fn output_shape_binding(
        self,
        db: &'db dyn fossil_base::Db,
    ) -> Option<(Option<SmolStr>, SmolStr)> {
        self.types(db)
            .iter()
            .find_map(|e| e.document.clone().map(|d| (e.constructor.clone(), d)))
    }

    /// The `(constructor, document)` of the binding that introduced
    /// `shape_iri` — the document a mapping targeting that shape is checked
    /// against.
    ///
    /// **By TYPE, not by file, and that was a bug with a fixture waiting for
    /// it.** This used to be [`Self::output_shape_binding`]: a `find_map` over
    /// the type bindings returning the FIRST document any of them named. With
    /// one document per program the two answers coincide; with two they do not,
    /// and `apps/docs/programs/multi-document` — `Persona` from `es.shex`,
    /// `Human` from `en.shex`, disjoint — resolved its second mapping against
    /// the first document and reported *«`es.shex` declares no shape
    /// `https://council.example/voc#Person`»*, which is true and is not the
    /// mistake. The document that declares a shape is the one that bound it, and
    /// `TypeEntry` has known which since it started carrying `document`.
    ///
    /// A shape NO binding introduced falls back to the first document, and that
    /// is deliberate: it is the misspelt-shape case, where every document is
    /// equally wrong and what the author needs is the list of what one of them
    /// declares (`TargetShapeError::Undeclared` carries it, with a did-you-mean
    /// over it).
    #[must_use]
    pub fn shape_binding_for(
        self,
        db: &'db dyn fossil_base::Db,
        shape_iri: &str,
    ) -> Option<(Option<SmolStr>, SmolStr)> {
        self.types(db)
            .iter()
            .find(|e| e.shape_iri.as_deref() == Some(shape_iri))
            .and_then(|e| e.document.clone().map(|d| (e.constructor.clone(), d)))
            .or_else(|| self.output_shape_binding(db))
    }
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn def_map<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> DefMap<'db> {
    let cst = fossil_syntax::parse(db, file);
    let mut sources: Vec<SourceEntry<'db>> = Vec::new();
    let mut mappings: Vec<MappingLoc<'db>> = Vec::new();
    let mut types: Vec<TypeEntry> = Vec::new();

    // Per-kind dense indices.
    //
    // CONTRACT: `MappingLoc.index` is the position
    // of the mapping among MAPPING-kind CST children only, NOT among all
    // top-level children (which would include SOURCE_DEF /
    // MULTI_SOURCE_DEF / TYPE_DEF). This MUST match the
    // ordering convention used by `crate::body::body`, which resolves a
    // mapping's body via
    //   cst.root(db).syntax().children()
    //      .filter(|n| n.kind() == SyntaxKind::MAPPING)
    //      .nth(loc.index(db))
    // i.e. filter-then-nth over MAPPING-kind nodes. If THIS loop were
    // ever changed to use the all-children index (e.g. via `.enumerate()`
    // on the unfiltered `.children()` iterator), `body()` would silently
    // resolve to the wrong CST subtree for any file with non-MAPPING
    // top-level siblings before the target mapping. The
    // `body_filters_to_mapping_kind_before_indexing` regression test in
    // `body.rs` enforces this contract end-to-end.
    //
    // SOURCE_DEF indexing follows the same per-kind dense scheme for
    // symmetry; no current downstream consumer depends on the all-children
    // index space for SOURCE_DEF either.
    let mut mapping_idx = 0usize;
    let mut source_idx = 0usize;
    for item in cst.root(db).syntax().children() {
        use fossil_syntax::SyntaxKind;
        match item.kind() {
            SyntaxKind::SOURCE_DEF => {
                if let Some(name) = parse_source_name(&item) {
                    let schema = parse_schema_arg(&item);
                    let (constructor, uri) = parse_source_call(&item);
                    sources.push(SourceEntry {
                        name,
                        loc: SourceLoc::new(db, file, source_idx),
                        schema_arg: schema.as_ref().and_then(|s| s.document.clone()),
                        schema_provider: schema.as_ref().and_then(|s| s.provider.clone()),
                        shape_iri: None,
                        // A single binding names no shape, so it cannot fail to
                        // bind one. Absence here is not an error.
                        shape_error: None,
                        constructor,
                        uri,
                        span: item_span(&item),
                    });
                    source_idx += 1;
                }
            }
            SyntaxKind::TYPE_DEF => {
                // `type { Person, City } := io.shex("personas.shex")`. Binds
                // TYPES, not sources, so it produces no `SourceEntry` and takes
                // no slot in `source_idx` — nothing downstream may read a type
                // name as a source. The document is the constructor's first
                // positional string, exactly as a source's URI is.
                let members = parse_brace_member_names(&item);
                let (ctor, document) = parse_source_call(&item);
                let bound = resolve_member_shape_iris(
                    db,
                    file,
                    ctor.as_deref(),
                    document.as_deref(),
                    &members,
                );
                // The `@rename` attributes above this binding, sorted by the
                // name each addresses. A rename addressed to a name the binding
                // does not introduce belongs to nobody and is dropped HERE —
                // `crate::lower` reports it, over the same nodes, with a real
                // span. This query stays diagnostic-free (it is signatures-only
                // and runs outside a frame where `delay_span_bug` is valid).
                let renames = parse_renames(&item);
                for (name, (shape_iri, shape_error)) in members.into_iter().zip(bound) {
                    let mine = renames
                        .iter()
                        .filter(|r| r.type_name == name)
                        .map(|r| (r.predicate.clone(), r.rename_to.clone()))
                        .collect();
                    types.push(TypeEntry {
                        name,
                        shape_iri,
                        shape_error,
                        document: document.clone(),
                        constructor: ctor.clone(),
                        renames: mine,
                    });
                }
            }
            SyntaxKind::MULTI_SOURCE_DEF => {
                // `{ A, B, ... } := io.rdf(uri, schema = io.shex("x.shex"))`. Each member
                // becomes its own `SourceEntry` sharing the one source's URI +
                // constructor + schema; the member's local-name resolves to a
                // shape IRI in the schema (COMPILE TIME). Each member then lowers
                // to its own `Op::Source` and `from <member>` resolves uniformly
                // via the existing per-source lookups.
                let members = parse_brace_member_names(&item);
                let schema = parse_schema_arg(&item);
                let schema_arg = schema.as_ref().and_then(|s| s.document.clone());
                let schema_provider = schema.as_ref().and_then(|s| s.provider.clone());
                let (constructor, uri) = parse_source_call(&item);
                // The PROVIDER the argument names, which is what selects the row
                // that reads the document. `None` is a program that wrote a bare
                // string, and it is an error rather than a fallback to the
                // extension — `crate::lower::check_provider` reports it.
                let shape_iris = resolve_member_shape_iris(
                    db,
                    file,
                    schema_provider.as_deref(),
                    schema_arg.as_deref(),
                    &members,
                );
                for (member, (shape_iri, shape_error)) in members.into_iter().zip(shape_iris) {
                    sources.push(SourceEntry {
                        name: member,
                        loc: SourceLoc::new(db, file, source_idx),
                        schema_arg: schema_arg.clone(),
                        schema_provider: schema_provider.clone(),
                        shape_iri,
                        shape_error,
                        constructor: constructor.clone(),
                        uri: uri.clone(),
                        // Every member of `{ A, B } := io.rdf(…)` shares the
                        // binding's span, because they share the binding. A
                        // per-member range would need the brace list's tokens
                        // and nothing asks for one.
                        span: item_span(&item),
                    });
                    source_idx += 1;
                }
            }
            SyntaxKind::MAPPING => {
                mappings.push(MappingLoc::new(db, file, mapping_idx));
                mapping_idx += 1;
            }
            _ => {}
        }
    }

    DefMap::new(db, sources, mappings, types)
}

// `parse_prefix_decl_node` lived here and read a `PREFIX_DECL`'s `IDENT` and
// `ABS_IRI` tokens into a `(name, iri)` pair. All three — the node kind and
// both token kinds — are gone.

/// Extract the bound name from a `SOURCE_DEF` node (the `users` in
/// `users := io.csv("...")`). The first IDENT child token is the binding name;
/// IDENTs nested inside the call expression belong to the callee.
/// The file-absolute [`Span`] of a top-level item, **trivia trimmed**.
///
/// `def_map` walks `cst.root(db)`, which is the whole file, so `text_range()`
/// here is already absolute — unlike the per-mapping subtrees rowan resets to
/// zero (`crate::spans`'s «offset semantics» section).
///
/// The trim is not cosmetic. A node's range carries the trailing newline, the
/// blank line after it and any comment in between, so an untrimmed span ends on
/// a LATER line and miette draws the label as a multi-line block — `╭─▶` down
/// the gutter, with the caret under an empty line. Measured twice: on
/// `fossil-cli`'s `broken_field` golden, and on `compound-key` with its join
/// condition broken, where the `// #endregion on` comment and the blank line
/// after it both ended up underlined.
///
/// Shared with [`crate::lower::lower_pipe_expr`] for that second one:
/// `HirSourcePipe::span` is «the whole `name := …` item» by the same
/// definition, and two implementations of one trim are two answers.
pub(crate) fn item_span(node: &fossil_syntax::SyntaxNode) -> Span {
    use fossil_syntax::SyntaxKind;

    // By TOKEN and not by trimming the text, because a comment is trivia and is
    // not whitespace. `Joined := LineRow.join(…)` followed by `// #endregion on`
    // carries that comment inside its own range, so trimming characters left it
    // underlined — `compound-key`, measured.
    let mut tokens = node
        .descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .map(|t| t.text_range());
    let Some(first) = tokens.next() else {
        // Nothing but trivia. The node's own range is the only answer left, and
        // a caller that underlines it is no worse off than before.
        let range = node.text_range();
        return Span::new(u32::from(range.start()), u32::from(range.end()));
    };
    let last = tokens.last().unwrap_or(first);
    Span::new(u32::from(first.start()), u32::from(last.end()))
}

fn parse_source_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    let ident = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    Some(SmolStr::from(ident.text()))
}

/// The `schema = <provider call>` argument of a source binding.
///
/// `{ A, B } := io.rdf("g.ttl", schema = io.shex("x.shex"))` →
/// `provider: "io.shex"`, `document: "x.shex"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SchemaArg {
    /// The dotted callee — `io.shex`, `io.shacl`. `None` when the value is not
    /// a call at all, which is the bare-string spelling and an error.
    pub provider: Option<SmolStr>,
    /// The document path the call names.
    pub document: Option<SmolStr>,
    /// The VALUE's own span, so the diagnostic underlines what was written and
    /// not the whole binding. This struct exists for that: the checking side
    /// (`crate::lower`) may not scan tokens of its own — there is one scanner
    /// for this argument and it is here, beside the other two.
    pub span: fossil_base::Span,
}

/// Extract the `schema =` argument from a `SOURCE_DEF` / `MULTI_SOURCE_DEF`
/// node's call expression, if it has one.
///
/// This reads ONLY the header tokens (the call expression on the right of
/// `:=`), never any mapping body — so it stays signatures-only and does not
/// widen the per-mapping `body()` fan-out, which is what keeps
/// [`crate::infer::resolve_source_scope`] out of `parse(file)`.
///
/// # It used to take a bare string, and that was the last hole in the rule
///
/// `schema = "x.shex"` named a document and no provider, so the decoder came
/// from the document's own EXTENSION — the one dispatch left that did not go by
/// name. The value is a provider call now, read by the same scan as any other
/// (`io` `.` `shex` `(` `"…"` `)`), so a document is never named without the row
/// that reads it. A value that is not a call yields `provider: None`, which is
/// reported with the span below rather than quietly falling back.
///
/// Heuristic token scan (the parser's `NAMED_ARG` / call surface is not yet a
/// stable structured node): find the `IDENT` `schema`, skip one `=`, then read
/// the dotted callee run and the first `STRING` that follows it.
pub(crate) fn parse_schema_arg(node: &fossil_syntax::SyntaxNode) -> Option<SchemaArg> {
    use fossil_syntax::SyntaxKind;
    let toks = non_trivia_tokens(node);
    let at = toks
        .iter()
        .position(|t| t.kind() == SyntaxKind::IDENT && t.text() == "schema")?;
    // The value begins after the `=`; a degenerate `schema "x"` (no binder) is
    // still read, because the parser may have recovered one away.
    let start = if toks
        .get(at + 1)
        .is_some_and(|t| matches!(t.kind(), SyntaxKind::ASSIGN | SyntaxKind::EQ))
    {
        at + 2
    } else {
        at + 1
    };
    let value = &toks[start.min(toks.len())..];
    let (provider, document) = parse_call_tokens(value);
    // The value's extent. A CALL runs to its own closing paren; a BARE STRING is
    // one token and stops there — taking the first `RPAREN` for that case too
    // would swallow the OUTER call's `)` and underline `"x.shex")`, which is not
    // what the author wrote wrong.
    let first = value.first()?;
    let last = if provider.is_some() {
        value
            .iter()
            .position(|t| t.kind() == SyntaxKind::RPAREN)
            .and_then(|i| value.get(i))
            .unwrap_or(first)
    } else {
        first
    };
    Some(SchemaArg {
        provider,
        document,
        span: fossil_base::Span::new(
            first.text_range().start().into(),
            last.text_range().end().into(),
        ),
    })
}

/// The node's tokens with whitespace, newlines and comments dropped — the shape
/// every scan in this module reads.
fn non_trivia_tokens(node: &fossil_syntax::SyntaxNode) -> Vec<fossil_syntax::SyntaxToken> {
    use fossil_syntax::SyntaxKind;
    node.descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect()
}

/// Extract the source constructor's dotted callee name and first positional
/// string argument from a `SOURCE_DEF` node:
/// `users := io.csv("examples/users.csv")` → `(Some("io.csv"),
/// Some("examples/users.csv"))`.
///
/// Like [`parse_schema_arg`] this reads ONLY the `SOURCE_DEF` header
/// tokens (the call expression on the right of `:=`), never any mapping body, so it
/// is signatures-only and does NOT widen the per-mapping `body()`
/// fan-out. The `def_map` query is file-keyed and structurally stable across
/// body-only edits (`tests/invalidation_regression.rs`).
///
/// Heuristic token scan (the parser's call surface is not yet a stable
/// structured node):
/// - the callee is the dotted run of `IDENT`s separated by `DOT` that begins
///   AFTER the `ASSIGN` token (skips the bound name's IDENT before `:=`);
/// - the URI is the FIRST `STRING` token, but only when it is POSITIONAL — a
///   `STRING` immediately preceded by `=`/`ASSIGN` is a named-argument value
///   (e.g. `schema = io.shex("x.shex")`) and is skipped.
pub(crate) fn parse_source_call(
    node: &fossil_syntax::SyntaxNode,
) -> (Option<SmolStr>, Option<SmolStr>) {
    parse_call_after(node, fossil_syntax::SyntaxKind::DEFINE)
}

/// [`parse_source_call`] with the binder spelled out.
///
/// It took a `binder` parameter because a `TYPE_DEF` used `=` where a
/// `SOURCE_DEF` used `:=`. There is ONE binder now: `:=` binds a name
/// (grammar.bnf, DEFINE) and `=` assigns a value (grammar.bnf, ASSIGN), and what
/// is being bound is read off the left-hand side rather than off the glyph. Both callers
/// pass `DEFINE`, so the parameter is one call away from being a constant — it
/// stays spelled out because the FIRST occurrence being the binder is the fact
/// this function rests on, and later `ASSIGN`s inside the call are
/// named-argument separators the URI scan below already knows to skip.
fn parse_call_after(
    node: &fossil_syntax::SyntaxNode,
    binder: fossil_syntax::SyntaxKind,
) -> (Option<SmolStr>, Option<SmolStr>) {
    let toks = non_trivia_tokens(node);
    // The binder separates the bound name(s) from the RHS call.
    let Some(define_pos) = toks.iter().position(|t| t.kind() == binder) else {
        return (None, None);
    };
    parse_call_tokens(&toks[define_pos + 1..])
}

/// `(callee, first positional string)` of a call written out as tokens.
///
/// One scanner, three callers: the right-hand side of a binding, the right-hand
/// side of a `type` binding, and now the value of `schema =` — which is a
/// provider call like the other two and is read exactly like them. Writing a
/// second scan for the argument is how `schema =` came to be dispatched by a
/// different criterion in the first place.
///
/// - the callee is the leading dotted run of `IDENT`s separated by `DOT`;
/// - the URI is the FIRST `STRING`, but only when it is POSITIONAL — a `STRING`
///   immediately preceded by `=`/`ASSIGN` is a named-argument value and is
///   skipped, which is what keeps `io.csv("u.csv", schema = …)` from reading the
///   argument's document as the source's URI.
fn parse_call_tokens(toks: &[fossil_syntax::SyntaxToken]) -> (Option<SmolStr>, Option<SmolStr>) {
    use fossil_syntax::SyntaxKind;

    let mut constructor = String::new();
    let mut expect_ident = true;
    for t in toks {
        match t.kind() {
            SyntaxKind::IDENT if expect_ident => {
                constructor.push_str(t.text());
                expect_ident = false;
            }
            SyntaxKind::DOT if !expect_ident => {
                constructor.push('.');
                expect_ident = true;
            }
            _ => break,
        }
    }
    let constructor = if constructor.is_empty() {
        None
    } else {
        Some(SmolStr::from(constructor))
    };

    let mut uri = None;
    for (i, t) in toks.iter().enumerate() {
        if t.kind() == SyntaxKind::STRING {
            let preceded_by_eq = i
                .checked_sub(1)
                .and_then(|p| toks.get(p))
                .is_some_and(|p| matches!(p.kind(), SyntaxKind::ASSIGN | SyntaxKind::EQ));
            if !preceded_by_eq {
                let raw = t.text();
                let inner = raw.trim_start_matches('"').trim_end_matches('"');
                uri = Some(SmolStr::from(inner));
                break;
            }
        }
    }

    (constructor, uri)
}

/// One `"…" as name` read off a `RENAME_ATTR`, with the name it is addressed
/// to.
///
/// Public to the crate because [`crate::lower`] validates the same three fields
/// against the binding and the shape document, with the CST node in hand so the
/// diagnostic has a real span. Two readers, one shape, and the reading is here
/// because this is where the CST is already being walked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameAttr {
    /// The `IDENT` after `@rename(` — one of the names the binding introduces.
    pub type_name: SmolStr,
    /// The predicate IRI, unquoted. A full IRI and never a short name: the two
    /// predicates being told apart differ only BEFORE their last segment, so
    /// the last segment cannot identify either.
    pub predicate: SmolStr,
    /// The `IDENT` after `as` — what to write in a property key instead.
    pub rename_to: SmolStr,
}

/// Every `RENAME_ATTR` above one `TYPE_DEF`, flattened to one entry per
/// `RENAME`.
///
/// `RenameAttr*` is part of the `TypeDef` production (grammar.bnf, `TypeDef`), so
/// these are CHILDREN of the node and not siblings of it — which is what makes a
/// rename belong to one binding rather than to the file.
///
/// **That distinction is load-bearing and it is not decoration.**
/// [`DefMap::output_shape_document`] returns the document of the FIRST `type`
/// binding in the file, a `find_map` over all of them; if a `@rename` were
/// file-scoped it would inherit that flattening and a two-binding program would
/// silently apply one binding's repair to the other's shape. Scoped to the node,
/// it cannot.
pub(crate) fn parse_renames(type_def: &fossil_syntax::SyntaxNode) -> Vec<RenameAttr> {
    use fossil_syntax::SyntaxKind;
    let mut out = Vec::new();
    for attr in type_def
        .children()
        .filter(|c| c.kind() == SyntaxKind::RENAME_ATTR)
    {
        // The attribute's own IDENT is the one that is NOT inside a `RENAME`:
        // `@rename(Person, "…" as foaf_name)` has `Person` as a direct token
        // child and `foaf_name` nested one level down.
        let Some(type_name) = attr
            .children_with_tokens()
            .filter_map(fossil_syntax::SyntaxElement::into_token)
            .find(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| SmolStr::from(t.text()))
        else {
            continue;
        };
        for rename in attr.children().filter(|c| c.kind() == SyntaxKind::RENAME) {
            let toks: Vec<_> = rename
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .filter(|t| {
                    !matches!(
                        t.kind(),
                        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
                    )
                })
                .collect();
            let predicate = toks.iter().find(|t| t.kind() == SyntaxKind::STRING);
            // The LAST `IDENT` is the new name; the first is the contextual
            // `as`, which is an ordinary identifier and lexes as one.
            let rename_to = toks.iter().rev().find(|t| t.kind() == SyntaxKind::IDENT);
            if let (Some(predicate), Some(rename_to)) = (predicate, rename_to)
                && rename_to.text() != "as"
            {
                out.push(RenameAttr {
                    type_name: type_name.clone(),
                    predicate: SmolStr::from(
                        predicate
                            .text()
                            .trim_start_matches('"')
                            .trim_end_matches('"'),
                    ),
                    rename_to: SmolStr::from(rename_to.text()),
                });
            }
        }
    }
    out
}

/// The brace-list names of a destructuring binding — the IDENTs strictly
/// between `{` and `}`.
///
/// Serves `MULTI_SOURCE_DEF` (`{ A, B } := …`) and `TYPE_DEF`
/// (`type { A, B } := …`) alike. Scoping to the braces rather than to "before
/// the binder" is what makes one function cover both: a `TYPE_DEF` carries the
/// contextual `type` IDENT before its `{`, and taking everything before the
/// binder would bind a phantom member called `type`.
fn parse_brace_member_names(node: &fossil_syntax::SyntaxNode) -> Vec<SmolStr> {
    use fossil_syntax::SyntaxKind;
    let toks: Vec<_> = node
        .descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect();
    let Some(open) = toks.iter().position(|t| t.kind() == SyntaxKind::LBRACE) else {
        return Vec::new();
    };
    let close = toks
        .iter()
        .position(|t| t.kind() == SyntaxKind::RBRACE)
        .unwrap_or(toks.len());
    toks[open + 1..close.max(open + 1)]
        .iter()
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))
        .collect()
}

/// Resolve each destructuring member to its shape IRI in the document, at
/// COMPILE TIME. A member that binds nothing carries WHY (the caller —
/// type-check — emits the diagnostic; this signatures-only query stays
/// diagnostic-free).
///
/// `constructor` is the provider the binding names (`io.shex`, `io.shacl`), or
/// `None` only when the program wrote a bare path, which is an error rather
/// than a fallback — see [`crate::shapes::decoded_document`].
fn resolve_member_shape_iris(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    constructor: Option<&str>,
    schema_arg: Option<&str>,
    members: &[SmolStr],
) -> Vec<(Option<SmolStr>, Option<ShapeBindError>)> {
    let all = |e: &ShapeBindError| {
        members
            .iter()
            .map(|_| (None, Some(e.clone())))
            .collect::<Vec<_>>()
    };
    let Some(schema_path) = schema_arg else {
        return all(&ShapeBindError::NoSchema);
    };
    let document = match crate::shapes::decoded_document(db, file, constructor, schema_path) {
        Ok(d) => d,
        Err(crate::shapes::DocumentError::Unregistered) => {
            return all(&ShapeBindError::Unreadable {
                path: SmolStr::from(schema_path),
                cause: SmolStr::from(crate::shapes::DocumentError::Unregistered.to_string()),
            });
        }
        // "we have it and cannot read it as a shape document" covers both a
        // document no decoder claims and one a decoder rejected.
        Err(e) => {
            return all(&ShapeBindError::Unparseable {
                path: SmolStr::from(schema_path),
                cause: SmolStr::from(e.to_string()),
            });
        }
    };

    // POSITIONAL: the Nth name binds the Nth shape the document declares. The
    // name is a free local label and is NOT looked up — which is why there is
    // no "matches no shape" case left, and why two documents can no longer
    // collide: two documents that both declare `Person` are told apart by the
    // local labels the program chose, not by the shape names.
    //
    // This is only sound because `OutputShapes::shapes()` yields declaration
    // order, which its own doc promises: a `HashMap` lived there until
    // `crates/fossil-shex/examples/declaration_order.rs` measured six parses
    // handing back six orders. Do not reintroduce a map here.
    let declared: Vec<SmolStr> = document
        .shapes()
        .map(|s| SmolStr::from(s.iri.as_str()))
        .collect();
    let arity = ShapeBindError::Arity {
        declared: declared.len(),
        named: members.len(),
    };
    (0..members.len())
        .map(|i| {
            declared.get(i).map_or_else(
                || (None, Some(arity.clone())),
                |iri| (Some(iri.clone()), None),
            )
        })
        .collect()
}

// `shape_local_name` lived here and cut a shape IRI at the last `#` or `/` so a
// member name could be matched against it. Positional binding retired it, and
// the bug it carried with it: two shapes from different vocabularies sharing a
// local name collapsed into one key of the lookup map, and one won in silence.

// `resolve_relative` lived here — "resolve a path this program wrote against
// the directory this program lives in" — and had one caller, `crate::shapes`,
// which immediately rendered the `PathBuf` back to the string `file_at` is
// keyed by. It is `crate::documents::registry_key` now, beside the loop the
// HOSTS call to register the same documents: the key and the lookup are one
// function, which is the only arrangement in which they cannot disagree.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    use fossil_base::test_support::{db_with_document, new_db};

    /// Two shapes, in this order, with nothing else in them. Positional binding
    /// is what these tests are about, so the names are deliberately words no
    /// binding below uses.
    const TWO_SHAPES: &str = "\
shape https://example.org/Zeta
shape https://example.org/Alpha
";

    const HELLO_FOSSIL: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name = User.name
";

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let db = new_db();
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    /// A binding's span is the binding, and stops at its last character.
    ///
    /// Sliced out of the source rather than compared to two numbers, because
    /// the numbers are the thing under test. `HELLO_FOSSIL` puts a blank line
    /// after this binding on purpose: rowan's `text_range()` carries the
    /// trailing trivia, so an untrimmed span ends on line 4 and miette renders
    /// the label as a multi-line block down the gutter, with the caret under
    /// nothing. `fossil-cli`'s `broken_field` golden showed exactly that.
    #[test]
    fn a_source_binding_span_is_the_binding_and_no_trivia() {
        let (db, file) = db_with_hello();
        let span = def_map(&db, file)
            .lookup_source_span(&db, "User")
            .expect("`User` is bound");
        let text = &HELLO_FOSSIL[span.start as usize..span.end as usize];
        assert_eq!(text, "User := io.csv(\"examples/users.csv\")");
    }

    /// And a COMMENT is trivia too, which trimming characters cannot see.
    ///
    /// The corpus writes these: `// #region` / `// #endregion` mark the runs
    /// `apps/docs` transcludes, so the binding a documentation page shows is
    /// exactly the binding with a comment glued to its range. Measured on
    /// `compound-key` with its join condition broken — the report underlined
    /// `// #endregion on` and the blank line after it, as a multi-line block.
    #[test]
    fn a_trailing_comment_is_trivia_and_stays_out_of_the_span() {
        const COMMENTED: &str = "\
type { Person } := io.shex(\"personas.shex\")

// #region binding
User := io.csv(\"examples/users.csv\")
// #endregion binding

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name = User.name
";
        let db = new_db();
        let file = fossil_base::SourceFile::new(
            &db,
            COMMENTED.to_string(),
            "examples/commented.fossil".to_string(),
        );
        let span = def_map(&db, file)
            .lookup_source_span(&db, "User")
            .expect("`User` is bound");
        assert_eq!(
            &COMMENTED[span.start as usize..span.end as usize],
            "User := io.csv(\"examples/users.csv\")"
        );
    }

    #[test]
    fn def_map_for_hello_fossil_has_one_type_one_source_one_mapping() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        // The prefix table is gone and the TYPE table is what replaced it as
        // the thing a shape name resolves against.
        assert_eq!(dm.types(&db).len(), 1);
        assert_eq!(dm.types(&db)[0].name, "Person");
        assert_eq!(
            dm.output_shape_document(&db).as_deref(),
            Some("personas.shex")
        );
        assert_eq!(dm.sources(&db).len(), 1);
        assert!(dm.lookup_source(&db, "User").is_some());
        assert_eq!(dm.mappings(&db).len(), 1);
    }

    #[test]
    fn def_map_resolves_source_constructor_and_uri() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let (ctor, uri) = dm.lookup_source_call(&db, "User").expect("User is bound");
        assert_eq!(ctor.as_deref(), Some("io.csv"));
        assert_eq!(uri.as_deref(), Some("examples/users.csv"));
    }

    #[test]
    fn def_map_resolves_json_and_parquet_constructors() {
        let src = "\
a := io.json(\"a.json\")
b := io.parquet(\"b.parquet\")
";
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        assert_eq!(
            dm.lookup_source_call(&db, "a"),
            Some((Some("io.json".into()), Some("a.json".into())))
        );
        assert_eq!(
            dm.lookup_source_call(&db, "b"),
            Some((Some("io.parquet".into()), Some("b.parquet".into())))
        );
    }

    #[test]
    fn def_map_skips_named_arg_string_for_uri() {
        // The POSITIONAL URI is resolved, the `schema = "..."` named-arg string
        // is NOT mistaken for the URI.
        let src = "u := io.csv(\"u.csv\", schema = io.shex(\"u.shex\"))\n";
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let (ctor, uri) = dm.lookup_source_call(&db, "u").unwrap();
        assert_eq!(ctor.as_deref(), Some("io.csv"));
        assert_eq!(uri.as_deref(), Some("u.csv"));
        assert_eq!(dm.lookup_source_schema(&db, "u").as_deref(), Some("u.shex"));
    }

    /// **`schema =` names a provider.** The argument carries the row that reads
    /// the document, so no document in the language arrives without one — and
    /// the source's own positional URI is still the source's, not the argument's.
    #[test]
    fn the_schema_argument_carries_its_provider() {
        let src = "{ a } := io.rdf(\"g.ttl\", schema = io.shex(\"two_shapes.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);
        let entry = &dm.sources(&db)[0];
        assert_eq!(entry.constructor.as_deref(), Some("io.rdf"));
        assert_eq!(entry.uri.as_deref(), Some("g.ttl"));
        assert_eq!(entry.schema_provider.as_deref(), Some("io.shex"));
        assert_eq!(entry.schema_arg.as_deref(), Some("two_shapes.shex"));
        assert!(dm.lookup_source_shape_iri(&db, "a").is_some());
    }

    /// A bare path still PARSES — the parser recovers it rather than dropping
    /// the argument — and comes back with no provider, which is what
    /// `crate::lower::check_schema_arg` reports with the argument's own span.
    /// Reading it as "no argument" would be the silent drop again.
    #[test]
    fn a_bare_schema_path_parses_and_names_no_provider() {
        let src = "{ a } := io.rdf(\"g.ttl\", schema = \"two_shapes.shex\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);
        let entry = &dm.sources(&db)[0];
        assert_eq!(entry.schema_arg.as_deref(), Some("two_shapes.shex"));
        assert_eq!(entry.schema_provider, None);
        // The span is the bare path and nothing else — not the outer call's
        // closing paren, which is the token immediately after it.
        let arg = parse_schema_arg(
            &fossil_syntax::parse(&db, file)
                .root(&db)
                .syntax()
                .children()
                .next()
                .expect("the binding"),
        )
        .expect("a `schema =` argument");
        assert_eq!(
            &src[arg.span.start as usize..arg.span.end as usize],
            "\"two_shapes.shex\""
        );
        assert!(
            matches!(
                dm.lookup_source_shape_error(&db, "a"),
                Some(ShapeBindError::Unparseable { .. })
            ),
            "no provider named ⇒ no row ⇒ no shapes, and it says which"
        );
    }

    /// The document declares `Zeta` then `Alpha`. The binding names them
    /// `primero` and `segundo` — words that appear nowhere in it. Under the old
    /// by-name rule both would resolve to `None`; under positional binding the
    /// first name takes the first DECLARED shape, so the names being free is
    /// exactly what this asserts.
    #[test]
    fn a_destructuring_member_binds_by_position_and_its_name_is_free() {
        let src =
            "{ primero, segundo } := io.rdf(\"g.ttl\", schema = io.shex(\"two_shapes.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_source_shape_iri(&db, "primero").as_deref(),
            Some("https://example.org/Zeta"),
            "the first name takes the first declared shape, whatever it is called"
        );
        assert_eq!(
            dm.lookup_source_shape_iri(&db, "segundo").as_deref(),
            Some("https://example.org/Alpha")
        );
        assert_eq!(dm.lookup_source_shape_error(&db, "primero"), None);
    }

    /// Naming more shapes than the document declares is the check the positional
    /// model gives away free, and it reports the two counts rather than blaming
    /// the last name for being misspelt.
    #[test]
    fn naming_more_shapes_than_the_document_declares_is_an_arity_error() {
        let src = "{ a, b, c } := io.rdf(\"g.ttl\", schema = io.shex(\"two_shapes.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert!(
            dm.lookup_source_shape_iri(&db, "b").is_some(),
            "b is second"
        );
        assert_eq!(
            dm.lookup_source_shape_error(&db, "c"),
            Some(ShapeBindError::Arity {
                declared: 2,
                named: 3
            })
        );
    }

    /// The four causes used to be one `None` behind one message that blamed the
    /// member's name. A document that is not there is not a misspelt name.
    #[test]
    fn a_document_that_cannot_be_read_says_so_instead_of_blaming_the_name() {
        let src = "{ a } := io.rdf(\"g.ttl\", schema = io.shex(\"does_not_exist.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert!(matches!(
            dm.lookup_source_shape_error(&db, "a"),
            Some(ShapeBindError::Unreadable { .. })
        ));
    }

    /// A document that IS there and cannot be read as a shape document is a
    /// different cause from one that is not there at all — the decoder's own
    /// reason travels with it.
    #[test]
    fn a_document_the_decoder_rejected_is_unparseable_and_carries_the_reason() {
        let src = "{ a } := io.rdf(\"g.ttl\", schema = io.shex(\"broken.shex\"))\n";
        let (db, file) = db_with_document(src, "broken.shex", "!malformed no shapes here\n");
        let dm = def_map(&db, file);

        let Some(ShapeBindError::Unparseable { path, cause }) =
            dm.lookup_source_shape_error(&db, "a")
        else {
            panic!(
                "expected Unparseable, got {:?}",
                dm.lookup_source_shape_error(&db, "a")
            );
        };
        assert_eq!(path, "broken.shex");
        assert!(cause.contains("no shapes here"), "got {cause}");
    }

    /// And a constructor with no `schema =` has no document at all — a third
    /// distinct cause, not the same `None` as the other three.
    #[test]
    fn a_destructuring_source_without_a_schema_argument_says_that() {
        let src = "{ a } := io.rdf(\"g.ttl\")\n";
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_source_shape_error(&db, "a"),
            Some(ShapeBindError::NoSchema)
        );
    }

    /// `type { … } := io.shex(…)` reaches the `DefMap`, binds positionally like
    /// its value-side twin, and carries the document. The document is the datum
    /// that matters: it is where a CSV-sourced program declares its output
    /// shape.
    #[test]
    fn a_type_binding_reaches_the_def_map_and_binds_positionally() {
        let src = "type { uno, dos } := io.shex(\"two_shapes.shex\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_type(&db, "uno").as_deref(),
            Some("https://example.org/Zeta")
        );
        assert_eq!(
            dm.lookup_type(&db, "dos").as_deref(),
            Some("https://example.org/Alpha")
        );
        assert_eq!(
            dm.output_shape_document(&db).as_deref(),
            Some("two_shapes.shex")
        );
    }

    /// A type name is not a source name. Reading one as the other would let a
    /// mapping say `from Person` and get a row out of a shape document, so the
    /// two tables stay disjoint and `source_idx` never advances for a `TYPE_DEF`.
    #[test]
    fn a_type_name_is_not_a_source_and_does_not_take_a_source_slot() {
        let src = "type { Person } := io.shex(\"two_shapes.shex\")\n\
                   users := io.csv(\"u.csv\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert!(dm.lookup_source(&db, "Person").is_none());
        assert!(dm.lookup_type(&db, "users").is_none());
        assert_eq!(
            dm.lookup_source(&db, "users").map(|l| l.index(&db)),
            Some(0),
            "the type binding must not consume a source index"
        );
    }

    /// The arity check applies on the type side too, and `type` remains usable
    /// as an ordinary binding name in the same file.
    #[test]
    fn a_type_binding_reports_arity_and_type_is_still_a_usable_name() {
        let src = "type { a, b, c } := io.shex(\"two_shapes.shex\")\n\
                   type := io.csv(\"type.csv\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_type_error(&db, "c"),
            Some(ShapeBindError::Arity {
                declared: 2,
                named: 3
            })
        );
        assert_eq!(
            dm.lookup_source_call(&db, "type"),
            Some((Some("io.csv".into()), Some("type.csv".into()))),
            "`type` is contextual, so it is still a binding name"
        );
    }

    /// `@rename` reaches the `DefMap`, and it reaches it ADDRESSED: one `type`
    /// line binds several names, and each rename goes to the one it names.
    ///
    /// Sorting by name and not by file is the point. `output_shape_document` is
    /// a `find_map` over the file's bindings — it answers with the FIRST — so a
    /// file-scoped rename table would inherit that flattening and apply one
    /// binding's repair to another binding's shape without a word.
    #[test]
    fn a_rename_is_addressed_to_one_bound_name() {
        let src = "@rename(uno, \"https://example.org/Zeta#name\" as zeta_name)\n\
                   @rename(dos, \"https://example.org/Alpha#name\" as alpha_name)\n\
                   type { uno, dos } := io.shex(\"two_shapes.shex\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        let by_name = |n: &str| {
            dm.types(&db)
                .iter()
                .find(|t| t.name == n)
                .expect("bound")
                .renames
                .clone()
        };
        assert_eq!(
            by_name("uno"),
            vec![(
                SmolStr::from("https://example.org/Zeta#name"),
                SmolStr::from("zeta_name")
            )],
            "`uno`'s rename, and only `uno`'s"
        );
        assert_eq!(
            by_name("dos"),
            vec![(
                SmolStr::from("https://example.org/Alpha#name"),
                SmolStr::from("alpha_name")
            )]
        );
        // And the shape-keyed accessor the checker uses reaches the same table:
        // by the time a `ResolvedShape` exists the local name is behind it.
        assert_eq!(
            dm.renames_for_shape(&db, "https://example.org/Zeta"),
            by_name("uno")
        );
    }

    /// A rename addressed to a name the binding does not introduce belongs to
    /// nobody, and is dropped HERE — `crate::lower::check_renames` reports it
    /// over the same nodes, with a real span. This test pins the drop so the
    /// report stays the only thing standing between it and silence.
    #[test]
    fn a_rename_addressed_to_an_unbound_name_binds_to_nothing() {
        let src = "@rename(Persn, \"https://example.org/Zeta#name\" as z)\n\
                   type { Person } := io.shex(\"two_shapes.shex\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);
        assert!(
            dm.types(&db)[0].renames.is_empty(),
            "`Persn` is not `Person`, and a rename is not applied to a name it \
             did not address"
        );
    }

    #[test]
    fn def_map_source_loc_is_interned_per_file() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let src = dm.lookup_source(&db, "User").unwrap();
        // Per-kind dense indexing. `users` is the only
        // SOURCE_DEF in hello.fossil, so its dense index is 0.
        assert_eq!(src.index(&db), 0);
        assert_eq!(src.file(&db), file);
    }

    #[test]
    fn def_map_mapping_loc_indexes_mapping_position() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let mappings = dm.mappings(&db);
        assert_eq!(mappings.len(), 1);
        // Per-kind dense indexing. `User` is the only MAPPING
        // in hello.fossil, so its dense index is 0 (was 2 under the
        // legacy all-children index space).
        assert_eq!(mappings[0].index(&db), 0);
        assert_eq!(mappings[0].file(&db), file);
    }

    #[test]
    fn def_map_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let a = def_map(&db, file);
        let b = def_map(&db, file);
        // Salsa-tracked structs compare by interned id; identical inputs
        // must yield the same handle (memoisation hit).
        assert_eq!(a, b);
    }
}
