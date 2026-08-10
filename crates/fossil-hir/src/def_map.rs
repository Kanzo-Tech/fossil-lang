//! [`DefMap`] — per-file name resolution table.
//!
//! Holds the prefix table (`prefix ex: <...>`), the source bindings
//! (`users := io.csv("...")`), and the mapping list (one entry per `Name :
//! Shape from source` block). Populated by the [`def_map`] Salsa query, which
//! consumes the CST from [`fossil_syntax::parse`].
//!
//! Per RESEARCH.md §"Architecture Patterns" Pattern 2 + 3, mapping and source
//! locations are interned with a `'db` lifetime so downstream queries can be
//! keyed per-item (rust-analyzer's per-item Salsa fan-out). Even though the
//! Phase 1 example has only one mapping, the SHAPE of the query graph matters
//! for Phase 2's stable item tree (CORE-02) and Phase 6's LSP perf benchmark.
//!
//! # Storage choice
//!
//! Prefix and source tables are stored as insertion-ordered `Vec<(K, V)>`
//! pairs rather than the originally-planned `IndexMap<K, V>`. The workspace
//! `indexmap = "2"` pin does not implement [`std::hash::Hash`] on `IndexMap`,
//! and Salsa tracked-struct fields require Hash for memoisation. Insertion
//! order is preserved either way; callers that need O(1) lookup can build a
//! `HashMap` at the call site (or use the [`DefMap::lookup_prefix`] /
//! [`DefMap::lookup_source`] helpers added below).

use fossil_base::SourceFile;
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

/// One entry in the prefix table (`prefix ex: <https://example.org/>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct PrefixEntry {
    pub name: SmolStr,
    pub iri: SmolStr,
}

/// Why a destructuring member bound no shape.
///
/// Four causes used to collapse into one `None`, and the single diagnostic that
/// existed blamed the member's NAME for all of them — so an unreadable file
/// reported "matches no shape in the schema". They are separated here because
/// the reader cannot act on a message that names the wrong thing (ADR-0057,
/// tenth amendment, and the correction it carries).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum ShapeBindError {
    /// The constructor carries no `schema = "…"`, so there is no document.
    NoSchema,
    /// The document is named but could not be read.
    Unreadable { path: SmolStr, cause: SmolStr },
    /// The document was read but is not a `ShEx` schema we can parse.
    Unparseable { path: SmolStr, cause: SmolStr },
    /// The binding names more shapes than the document declares. Binding is
    /// POSITIONAL (tenth amendment), so this is the check that model gives
    /// away free: the Nth name wants an Nth shape and there is none.
    Arity { declared: usize, named: usize },
}

/// One entry in the source-binding table (`users := io.csv(...)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceEntry<'db> {
    pub name: SmolStr,
    pub loc: SourceLoc<'db>,
    /// Value of the source constructor's `schema = "<path>"` NAMED argument,
    /// if present (e.g. `users := io.csv("u.csv", schema = "users.csvw.json")`).
    ///
    /// Phase 3 plan 03-05 reads this to wire forward CSVW propagation
    /// ([`crate::infer::resolve_source_row`]). It is a SIGNATURE-only datum:
    /// it is read from the `SOURCE_DEF` header tokens, NOT from any mapping
    /// body, so it does NOT widen the per-mapping `body()` fan-out. The
    /// `def_map` query is file-keyed and structurally stable across
    /// body-only edits (verified by `tests/invalidation_regression.rs`).
    pub schema_arg: Option<SmolStr>,
    /// For a destructuring RDF source member (`{ IfcBeam, ... } := io.rdf(...,
    /// schema = "x.shex")`), the shape IRI this member's local-name resolves to
    /// in the schema (`IfcBeam` ↔ `http://ifcowl.../IfcBeam`), resolved at
    /// COMPILE TIME against the `ShEx`. Subject selection is always by `rdf:type ==
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
}

/// One name bound by a type binding (`type { Person, City } = io.shex("s.shex")`).
///
/// The value side of the language binds sources; this binds TYPES. One
/// catalogue (`io.*`), two binders (ADR-0057, seventh amendment).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeEntry {
    /// The local label. Free — it does not have to name anything in the
    /// document, because binding is by position (tenth amendment). This is
    /// what lets two documents that both declare `Person` coexist.
    pub name: SmolStr,
    /// The shape IRI this name binds: the Nth declared shape for the Nth name.
    pub shape_iri: Option<SmolStr>,
    /// Why it bound nothing, when it bound nothing. Never a bare `None`.
    pub shape_error: Option<ShapeBindError>,
    /// The document the constructor names — `io.shex("personas.shex")`. This is
    /// the datum that gives a CSV-sourced program somewhere to declare its
    /// output shape, which is the hole ADR-0055 named and left open.
    pub document: Option<SmolStr>,
}

#[salsa::tracked(debug)]
pub struct DefMap<'db> {
    #[returns(ref)]
    pub prefixes: Vec<PrefixEntry>,
    #[returns(ref)]
    pub sources: Vec<SourceEntry<'db>>,
    #[returns(ref)]
    pub mappings: Vec<MappingLoc<'db>>,
    #[returns(ref)]
    pub types: Vec<TypeEntry>,
}

impl<'db> DefMap<'db> {
    /// Look up the expanded IRI for a prefix name. O(n) over the prefix list;
    /// fine for Phase 1 (single-digit prefix counts in real `.fossil` files).
    /// Phase 2 may swap the storage for an interned `IndexMap` once the Salsa
    /// `Hash` constraint is resolved upstream.
    #[must_use]
    pub fn lookup_prefix(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SmolStr> {
        self.prefixes(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.iri.clone())
    }

    /// Look up the [`SourceLoc`] bound to a source name (e.g. `users`).
    #[must_use]
    pub fn lookup_source(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SourceLoc<'db>> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.loc)
    }

    /// Look up the `schema = "<path>"` argument bound to a source name, if the
    /// source declared one. Used by plan 03-05's
    /// [`crate::infer::resolve_source_row`] to wire forward CSVW propagation.
    #[must_use]
    pub fn lookup_source_schema(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SmolStr> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.schema_arg.clone())
    }

    /// Look up the resolved shape IRI bound to a destructuring RDF source member
    /// (`{ IfcBeam, ... } := io.rdf(..., schema = "x.shex")` → `IfcBeam`'s shape
    /// IRI). `None` for native single-binding sources. Used by the input typing
    /// ([`crate::infer::resolve_source_row`]) to resolve the member's
    /// compile-time row type from the `ShEx` shape.
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

    /// The shape IRI a `type { … } = io.shex(…)` name binds.
    #[must_use]
    pub fn lookup_type(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SmolStr> {
        self.types(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_iri.clone())
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
    /// is a CSV — the case that had nowhere to declare a shape at all, named as
    /// a hole by ADR-0055 and left open. One document per file for now: a second
    /// `type … =` is legal and binds its own names, but which document backs the
    /// output contract is not something any program has had to say yet.
    #[must_use]
    pub fn output_shape_document(self, db: &'db dyn fossil_base::Db) -> Option<SmolStr> {
        self.types(db).iter().find_map(|e| e.document.clone())
    }
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn def_map<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> DefMap<'db> {
    let cst = fossil_syntax::parse(db, file);
    let mut prefixes: Vec<PrefixEntry> = Vec::new();
    let mut sources: Vec<SourceEntry<'db>> = Vec::new();
    let mut mappings: Vec<MappingLoc<'db>> = Vec::new();
    let mut types: Vec<TypeEntry> = Vec::new();

    // Per-kind dense indices.
    //
    // CONTRACT (Plan 02-04 — ADR-0005): `MappingLoc.index` is the position
    // of the mapping among MAPPING-kind CST children only, NOT among all
    // top-level children (which would include PREFIX_DECL / SOURCE_DEF /
    // MULTI_SOURCE_DEF / IMPORT). This MUST match the
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
            SyntaxKind::PREFIX_DECL => {
                if let Some((name, iri)) = parse_prefix_decl_node(&item) {
                    prefixes.push(PrefixEntry { name, iri });
                }
            }
            SyntaxKind::SOURCE_DEF => {
                if let Some(name) = parse_source_name(&item) {
                    let schema_arg = parse_source_named_arg(&item, "schema");
                    let (constructor, uri) = parse_source_call(&item);
                    sources.push(SourceEntry {
                        name,
                        loc: SourceLoc::new(db, file, source_idx),
                        schema_arg,
                        shape_iri: None,
                        // A single binding names no shape, so it cannot fail to
                        // bind one. Absence here is not an error.
                        shape_error: None,
                        constructor,
                        uri,
                    });
                    source_idx += 1;
                }
            }
            SyntaxKind::TYPE_DEF => {
                // `type { Person, City } = io.shex("personas.shex")`. Binds TYPES,
                // not sources, so it produces no `SourceEntry` and takes no slot
                // in `source_idx` — nothing downstream may read a type name as a
                // source. The document is the constructor's first positional
                // string, exactly as a source's URI is.
                let members = parse_brace_member_names(&item);
                let (_ctor, document) = parse_call_after(&item, SyntaxKind::ASSIGN);
                let bound = resolve_member_shape_iris(db, file, document.as_deref(), &members);
                for (name, (shape_iri, shape_error)) in members.into_iter().zip(bound) {
                    types.push(TypeEntry {
                        name,
                        shape_iri,
                        shape_error,
                        document: document.clone(),
                    });
                }
            }
            SyntaxKind::MULTI_SOURCE_DEF => {
                // `{ A, B, ... } := io.rdf(uri, schema = "x.shex")`. Each member
                // becomes its own `SourceEntry` sharing the one source's URI +
                // constructor + schema; the member's local-name resolves to a
                // shape IRI in the schema (COMPILE TIME). Each member then lowers
                // to its own `Op::Source` and `from <member>` resolves uniformly
                // via the existing per-source lookups.
                let members = parse_brace_member_names(&item);
                let schema_arg = parse_source_named_arg(&item, "schema");
                let (constructor, uri) = parse_source_call(&item);
                let shape_iris =
                    resolve_member_shape_iris(db, file, schema_arg.as_deref(), &members);
                for (member, (shape_iri, shape_error)) in members.into_iter().zip(shape_iris) {
                    sources.push(SourceEntry {
                        name: member,
                        loc: SourceLoc::new(db, file, source_idx),
                        schema_arg: schema_arg.clone(),
                        shape_iri,
                        shape_error,
                        constructor: constructor.clone(),
                        uri: uri.clone(),
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

    DefMap::new(db, prefixes, sources, mappings, types)
}

/// Extract `(prefix-name, expanded-IRI)` from a `PREFIX_DECL` node.
///
/// Mirrors the AST view's [`fossil_syntax::ast::PrefixDecl`] accessor logic
/// but operates directly on the `SyntaxNode` to keep the lowering self-
/// contained (no AST cast indirection on the Salsa-tracked path).
fn parse_prefix_decl_node(node: &fossil_syntax::SyntaxNode) -> Option<(SmolStr, SmolStr)> {
    use fossil_syntax::SyntaxKind;
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .collect();
    let name = toks.iter().find(|t| t.kind() == SyntaxKind::IDENT)?;
    let abs_iri = toks.iter().find(|t| t.kind() == SyntaxKind::ABS_IRI)?;
    let iri_text = abs_iri.text().trim_start_matches('<').trim_end_matches('>');
    Some((SmolStr::from(name.text()), SmolStr::from(iri_text)))
}

/// Extract the bound name from a `SOURCE_DEF` node (the `users` in
/// `users := io.csv("...")`). The first IDENT child token is the binding name;
/// IDENTs nested inside the call expression belong to the callee.
fn parse_source_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    let ident = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    Some(SmolStr::from(ident.text()))
}

/// Extract a named `<arg_name> = "<path>"` argument from a `SOURCE_DEF` node's
/// call expression, if present (e.g. `schema` or `select`).
///
/// This reads ONLY the `SOURCE_DEF` header tokens (the call expression on the
/// right of `:=`), never any mapping body — so it stays signatures-only per
/// ADR-0005 and does not widen the per-mapping `body()` fan-out (Serious #6
/// mitigation for plan 03-05's `resolve_source_row`).
///
/// Heuristic token scan (the parser's `NAMED_ARG` / call surface is not
/// yet a stable structured node in Phase 3 v0.1): find an `IDENT` whose text is
/// `arg_name`, immediately followed (skipping trivia) by an `=`/assignment token
/// and then a `STRING` literal. Returns the unquoted string contents.
fn parse_source_named_arg(node: &fossil_syntax::SyntaxNode, arg_name: &str) -> Option<SmolStr> {
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
    for (i, t) in toks.iter().enumerate() {
        if t.kind() == SyntaxKind::IDENT && t.text() == arg_name {
            // Look ahead for a STRING within the next two non-trivia tokens
            // (covers both `schema = "x"` and a degenerate `schema "x"` form).
            if let Some(s) = toks
                .iter()
                .skip(i + 1)
                .take(2)
                .find(|t| t.kind() == SyntaxKind::STRING)
            {
                let raw = s.text();
                let inner = raw.trim_start_matches('"').trim_end_matches('"');
                return Some(SmolStr::from(inner));
            }
        }
    }
    None
}

/// Extract the source constructor's dotted callee name and first positional
/// string argument from a `SOURCE_DEF` node:
/// `users := io.csv("examples/users.csv")` → `(Some("io.csv"),
/// Some("examples/users.csv"))`.
///
/// Like [`parse_source_named_arg`] this reads ONLY the `SOURCE_DEF` header
/// tokens (the call expression on the right of `:=`), never any mapping body, so it
/// is signatures-only per ADR-0005 and does NOT widen the per-mapping `body()`
/// fan-out. The `def_map` query is file-keyed and structurally stable across
/// body-only edits (`tests/invalidation_regression.rs`).
///
/// Heuristic token scan (the parser's call surface is not yet a stable
/// structured node):
/// - the callee is the dotted run of `IDENT`s separated by `DOT` that begins
///   AFTER the `ASSIGN` token (skips the bound name's IDENT before `:=`);
/// - the URI is the FIRST `STRING` token, but only when it is POSITIONAL — a
///   `STRING` immediately preceded by `=`/`ASSIGN` is a named-argument value
///   (e.g. `schema = "users.csvw.json"`) and is skipped.
fn parse_source_call(node: &fossil_syntax::SyntaxNode) -> (Option<SmolStr>, Option<SmolStr>) {
    parse_call_after(node, fossil_syntax::SyntaxKind::DEFINE)
}

/// [`parse_source_call`] with the binder spelled out, because a `TYPE_DEF` uses
/// `=` where a `SOURCE_DEF` uses `:=` (ADR-0057: `:=` binds a value, `type … =`
/// binds a type). The FIRST occurrence is the binder — later `ASSIGN`s inside
/// the call are named-argument separators, which the URI scan below already
/// knows to skip.
fn parse_call_after(
    node: &fossil_syntax::SyntaxNode,
    binder: fossil_syntax::SyntaxKind,
) -> (Option<SmolStr>, Option<SmolStr>) {
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

    // The binder separates the bound name(s) from the RHS call.
    let Some(define_pos) = toks.iter().position(|t| t.kind() == binder) else {
        return (None, None);
    };
    let rhs = &toks[define_pos + 1..];

    // Callee: the leading dotted IDENT run (`io` `.` `csv` → `io.csv`).
    let mut constructor = String::new();
    let mut expect_ident = true;
    for t in rhs {
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

    // URI: the first POSITIONAL string in the RHS. A `STRING` whose immediately
    // preceding non-trivia token is `=`/`ASSIGN` is a named-arg value — skip it.
    let mut uri = None;
    for (i, t) in rhs.iter().enumerate() {
        if t.kind() == SyntaxKind::STRING {
            let preceded_by_eq = i
                .checked_sub(1)
                .and_then(|p| rhs.get(p))
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

/// The brace-list names of a destructuring binding — the IDENTs strictly
/// between `{` and `}`.
///
/// Serves `MULTI_SOURCE_DEF` (`{ A, B } := …`) and `TYPE_DEF`
/// (`type { A, B } = …`) alike. Scoping to the braces rather than to "before
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

/// Resolve each destructuring member's local-name to its shape IRI in the
/// schema, at COMPILE TIME. Reads the `ShEx` via the host filesystem (mirrors
/// [`crate::infer::resolve_source_row`]'s CSVW read), parses it, and matches
/// each member to the shape whose IRI local-name equals the member. A member
/// with no matching shape yields `None` (the caller — type-check — emits the
/// "unknown shape" diagnostic; this signatures-only query stays diagnostic-free).
fn resolve_member_shape_iris(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
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
    let resolved = resolve_relative(db, file, schema_path);
    let bytes = match db.system().read_file(&resolved) {
        Ok(b) => b,
        Err(e) => {
            return all(&ShapeBindError::Unreadable {
                path: SmolStr::from(schema_path),
                cause: SmolStr::from(e.to_string()),
            });
        }
    };
    let desc = match fossil_descriptors_output::ShExDescriptor::from_reader(bytes.as_slice()) {
        Ok(d) => d,
        Err(e) => {
            return all(&ShapeBindError::Unparseable {
                path: SmolStr::from(schema_path),
                cause: SmolStr::from(format!("{e:?}")),
            });
        }
    };

    // POSITIONAL: the Nth name binds the Nth shape the document declares. The
    // name is a free local label and is NOT looked up — which is why there is
    // no "matches no shape" case left, and why two documents can no longer
    // collide (ADR-0057, tenth amendment).
    //
    // This is only sound because `ShExDescriptor::shapes()` yields declaration
    // order. It did not until `fix(shex)`: it was a `HashMap`, and six parses
    // gave six orders. Do not reintroduce a map here.
    let declared: Vec<SmolStr> = desc
        .shapes()
        .map(|b| SmolStr::from(b.iri.to_string()))
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

/// Resolve `schema_path` relative to the directory containing `file`'s path.
/// Mirrors [`crate::infer`]'s resolver (kept local to avoid a cross-module pub).
fn resolve_relative(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    schema_path: &str,
) -> std::path::PathBuf {
    let file_path = std::path::PathBuf::from(file.path(db));
    file_path.parent().map_or_else(
        || std::path::PathBuf::from(schema_path),
        |dir| dir.join(schema_path),
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    #[test]
    fn def_map_for_hello_fossil_has_one_prefix_one_source_one_mapping() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        assert_eq!(dm.prefixes(&db).len(), 1);
        assert_eq!(
            dm.lookup_prefix(&db, "ex").as_deref(),
            Some("https://example.org/")
        );
        assert_eq!(dm.sources(&db).len(), 1);
        assert!(dm.lookup_source(&db, "users").is_some());
        assert_eq!(dm.mappings(&db).len(), 1);
    }

    #[test]
    fn def_map_resolves_source_constructor_and_uri() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let (ctor, uri) = dm.lookup_source_call(&db, "users").expect("users is bound");
        assert_eq!(ctor.as_deref(), Some("io.csv"));
        assert_eq!(uri.as_deref(), Some("examples/users.csv"));
    }

    #[test]
    fn def_map_resolves_json_and_parquet_constructors() {
        let src = "\
a := io.json(\"a.json\")
b := io.parquet(\"b.parquet\")
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
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
        let src = "u := io.csv(\"u.csv\", schema = \"u.csvw.json\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let (ctor, uri) = dm.lookup_source_call(&db, "u").unwrap();
        assert_eq!(ctor.as_deref(), Some("io.csv"));
        assert_eq!(uri.as_deref(), Some("u.csv"));
        assert_eq!(
            dm.lookup_source_schema(&db, "u").as_deref(),
            Some("u.csvw.json")
        );
    }

    /// The document declares `Zeta` then `Alpha`. The binding names them
    /// `primero` and `segundo` — words that appear nowhere in it. Under the old
    /// by-name rule both would resolve to `None`; under positional binding the
    /// first name takes the first DECLARED shape, so the names being free is
    /// exactly what this asserts (ADR-0057, tenth amendment).
    #[test]
    fn a_destructuring_member_binds_by_position_and_its_name_is_free() {
        let src = "{ primero, segundo } := io.rdf(\"g.ttl\", \
                   schema = \"tests/fixtures/positional_binding/two_shapes.shex\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
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
        let src = "{ a, b, c } := io.rdf(\"g.ttl\", \
                   schema = \"tests/fixtures/positional_binding/two_shapes.shex\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
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
        let src = "{ a } := io.rdf(\"g.ttl\", schema = \"tests/fixtures/does_not_exist.shex\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);

        assert!(matches!(
            dm.lookup_source_shape_error(&db, "a"),
            Some(ShapeBindError::Unreadable { .. })
        ));
    }

    /// And a constructor with no `schema =` has no document at all — a third
    /// distinct cause, not the same `None` as the other three.
    #[test]
    fn a_destructuring_source_without_a_schema_argument_says_that() {
        let src = "{ a } := io.rdf(\"g.ttl\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_source_shape_error(&db, "a"),
            Some(ShapeBindError::NoSchema)
        );
    }

    /// `type { … } = io.shex(…)` reaches the `DefMap`, binds positionally like
    /// its value-side twin, and carries the document. The document is the datum
    /// that matters: it is where a CSV-sourced program declares its output
    /// shape, which ADR-0055 named as a hole and left open.
    #[test]
    fn a_type_binding_reaches_the_def_map_and_binds_positionally() {
        let src = "type { uno, dos } = \
                   io.shex(\"tests/fixtures/positional_binding/two_shapes.shex\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
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
            Some("tests/fixtures/positional_binding/two_shapes.shex")
        );
    }

    /// A type name is not a source name. Reading one as the other would let a
    /// mapping say `from Person` and get a row out of a shape document, so the
    /// two tables stay disjoint and `source_idx` never advances for a `TYPE_DEF`.
    #[test]
    fn a_type_name_is_not_a_source_and_does_not_take_a_source_slot() {
        let src = "type { Person } = \
                   io.shex(\"tests/fixtures/positional_binding/two_shapes.shex\")\n\
                   users := io.csv(\"u.csv\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
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
        let src = "type { a, b, c } = \
                   io.shex(\"tests/fixtures/positional_binding/two_shapes.shex\")\n\
                   type := io.csv(\"type.csv\")\n";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
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

    #[test]
    fn def_map_source_loc_is_interned_per_file() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let src = dm.lookup_source(&db, "users").unwrap();
        // Plan 02-03: per-kind dense indexing. `users` is the only
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
        // Plan 02-03: per-kind dense indexing. `User` is the only MAPPING
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
