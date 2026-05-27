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
    /// Dotted name of the source constructor (`io.csv` / `io.json` /
    /// `io.parquet`), if a `CALL_EXPR`-shaped RHS could be parsed. The
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

#[salsa::tracked(debug)]
pub struct DefMap<'db> {
    #[returns(ref)]
    pub prefixes: Vec<PrefixEntry>,
    #[returns(ref)]
    pub sources: Vec<SourceEntry<'db>>,
    #[returns(ref)]
    pub mappings: Vec<MappingLoc<'db>>,
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
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn def_map<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> DefMap<'db> {
    let cst = fossil_syntax::parse(db, file);
    let mut prefixes: Vec<PrefixEntry> = Vec::new();
    let mut sources: Vec<SourceEntry<'db>> = Vec::new();
    let mut mappings: Vec<MappingLoc<'db>> = Vec::new();

    // Per-kind dense indices.
    //
    // CONTRACT (Plan 02-04 — ADR-0005): `MappingLoc.index` is the position
    // of the mapping among MAPPING-kind CST children only, NOT among all
    // top-level children (which would include PREFIX_DECL / SOURCE_DEF /
    // IMPORT / DEFINITION / EXPORTED_DEFINITION). This MUST match the
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
                    let schema_arg = parse_source_schema_arg(&item);
                    let (constructor, uri) = parse_source_call(&item);
                    sources.push(SourceEntry {
                        name,
                        loc: SourceLoc::new(db, file, source_idx),
                        schema_arg,
                        constructor,
                        uri,
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

    DefMap::new(db, prefixes, sources, mappings)
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
/// IDENTs nested inside `CALL_EXPR` belong to the callee.
fn parse_source_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    let ident = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)?;
    Some(SmolStr::from(ident.text()))
}

/// Extract the `schema = "<path>"` NAMED argument from a `SOURCE_DEF` node's
/// call expression, if present.
///
/// This reads ONLY the `SOURCE_DEF` header tokens (the call expression on the
/// right of `:=`), never any mapping body — so it stays signatures-only per
/// ADR-0005 and does not widen the per-mapping `body()` fan-out (Serious #6
/// mitigation for plan 03-05's `resolve_source_row`).
///
/// Heuristic token scan (the parser's `NAMED_ARG` / `CALL_EXPR` surface is not
/// yet a stable structured node in Phase 3 v0.1): find an `IDENT` whose text is
/// `schema`, immediately followed (skipping trivia) by an `=`/assignment token
/// and then a `STRING` literal. Returns the unquoted string contents.
fn parse_source_schema_arg(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
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
        if t.kind() == SyntaxKind::IDENT && t.text() == "schema" {
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
/// Like [`parse_source_schema_arg`] this reads ONLY the `SOURCE_DEF` header
/// tokens (the `CALL_EXPR` on the right of `:=`), never any mapping body, so it
/// is signatures-only per ADR-0005 and does NOT widen the per-mapping `body()`
/// fan-out. The `def_map` query is file-keyed and structurally stable across
/// body-only edits (`tests/invalidation_regression.rs`).
///
/// Heuristic token scan (the parser's `CALL_EXPR` surface is not yet a stable
/// structured node):
/// - the callee is the dotted run of `IDENT`s separated by `DOT` that begins
///   AFTER the `ASSIGN` token (skips the bound name's IDENT before `:=`);
/// - the URI is the FIRST `STRING` token, but only when it is POSITIONAL — a
///   `STRING` immediately preceded by `=`/`ASSIGN` is a named-argument value
///   (e.g. `schema = "users.csvw.json"`) and is skipped.
fn parse_source_call(node: &fossil_syntax::SyntaxNode) -> (Option<SmolStr>, Option<SmolStr>) {
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

    // Find the `:=` (DEFINE) that separates the bound name from the RHS call.
    // (`=`/`ASSIGN` is the NAMED-ARG separator, handled below — not this one.)
    let Some(define_pos) = toks.iter().position(|t| t.kind() == SyntaxKind::DEFINE) else {
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
