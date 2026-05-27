//! CST → HIR lowering.
//!
//! Phase 2 (plan 02-04, per ADR-0005) splits Phase 1's flat `HirMapping`
//! shape: header signature fields (name, shape IRI, source binding) live
//! here; the per-mapping property list moved into [`crate::body::HirBody`],
//! reached via the [`crate::body::body`] Salsa query keyed by
//! [`crate::def_map::MappingLoc`]. The split is the precondition for
//! CORE-02 SC#2: editing one property's right-hand side invalidates only
//! `body(M_k)` + its downstream queries, never the file-level
//! `item_tree(file)` or `lower_to_hir(file)` queries' structural inputs.
//!
//! The expression encoding remains intentionally minimal:
//! - [`HirExpr::Template`] keeps the raw backtick text including `${...}`
//!   placeholders. Codegen parses the template at SQL-emission time. Phase 4
//!   lifts template parsing into a real expression tree.
//! - [`HirExpr::FieldRef`] is just the field name (`.id` → `"id"`).
//! - [`HirExpr::PrefixedName`] carries the already-resolved full IRI.
//! - [`HirExpr::StringLit`] holds the literal text without surrounding quotes.

use fossil_base::{SourceFile, Span, delay_span_bug};
use smol_str::SmolStr;

use crate::def_map::{PrefixEntry, def_map};

#[salsa::tracked(debug)]
pub struct HirFile<'db> {
    #[returns(ref)]
    pub mappings: Vec<HirMapping>,
}

/// Per-mapping HEADER data. Per ADR-0005, the previous `properties` field
/// is REMOVED — body content lives in [`crate::body::HirBody`], reached via
/// [`crate::body::body`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirMapping {
    /// Mapping name, e.g. `"User"`.
    pub name: SmolStr,
    /// Fully-resolved shape IRI, e.g. `"https://example.org/Person"`.
    pub shape_iri: SmolStr,
    /// Name of the source binding referenced by `from`, e.g. `"users"`.
    pub source_binding: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirProperty {
    pub key: PropertyKey,
    pub value: HirExpr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum PropertyKey {
    /// `iri = ...` — the special "subject IRI" property of a mapping.
    Iri,
    /// `ex:name = ...` — predicate IRI built from `prefix:local` resolved
    /// against the per-file [`crate::DefMap`] prefix table.
    PrefixedName { iri: SmolStr },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum HirExpr {
    /// Backtick template, raw text including delimiters and `${...}`
    /// placeholders. Phase 4 lifts template parsing into a real expression
    /// tree; Phase 1 codegen parses the template at SQL-emission time.
    Template(SmolStr),
    /// `.id` → field name `"id"`.
    FieldRef(SmolStr),
    /// `"hello"` → literal text without surrounding quotes.
    StringLit(SmolStr),
    /// `ex:foo` resolved to its full IRI.
    PrefixedName { iri: SmolStr },
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn lower_to_hir<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> HirFile<'db> {
    let cst = fossil_syntax::parse(db, file);
    let dm = def_map(db, file);
    let prefixes = dm.prefixes(db);

    let mut mappings = Vec::new();
    for child in cst.root(db).syntax().children() {
        if child.kind() == fossil_syntax::SyntaxKind::MAPPING
            && let Some(m) = lower_mapping_node(&child, prefixes)
        {
            mappings.push(m);
        }
    }
    HirFile::new(db, mappings)
}

fn lookup_prefix(prefixes: &[PrefixEntry], name: &str) -> Option<SmolStr> {
    prefixes
        .iter()
        .find(|e| e.name.as_str() == name)
        .map(|e| e.iri.clone())
}

fn lower_mapping_node(
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirMapping> {
    use fossil_syntax::SyntaxKind;

    let header = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)?;
    // Per ADR-0005, properties are NOT collected here — the body() Salsa
    // query owns them. The MAPPING_BODY's presence is no longer required for
    // a successful header lowering; an empty-bodied mapping is still a valid
    // HirMapping signature.

    // Phase 2 plan 02-03 wraps the header's shape and source in composite
    // sub-nodes:
    //
    //   MAPPING_HEADER
    //     IDENT "User"                      -- direct token: mapping name
    //     SHAPE_SEP ":"
    //     SHAPE_EXPR
    //       IRI_EXPR
    //         IDENT "ex" SHAPE_SEP ":" IDENT "Person"
    //     (optional) IN_CLAUSE
    //     KW_FROM "from"
    //     EXPR
    //       LITERAL_EXPR
    //         IDENT "users"                 -- the from-source expression
    //
    // Phase 1's flat "first four IDENTs" shortcut no longer matches; walk the
    // sub-nodes by kind to extract each header field cleanly.
    let name = header
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))?;

    // ShapeExpr → first IRI_EXPR → its (prefix-name, local-name) tokens. For
    // the Phase 1 hello.fossil + Wave 1 fixtures this is the lexer-contiguous
    // `IDENT SHAPE_SEP IDENT` shape; degenerate single-IDENT IRIExprs (from
    // the recovery path) cause us to bail with `None` (Phase 3 promotes to
    // a real diagnostic).
    let shape_expr = header
        .children()
        .find(|c| c.kind() == SyntaxKind::SHAPE_EXPR)?;
    let first_iri = shape_expr
        .children()
        .find(|c| c.kind() == SyntaxKind::IRI_EXPR)?;
    let iri_idents: Vec<_> = first_iri
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .collect();
    if iri_idents.len() < 2 {
        return None;
    }
    let shape_prefix_name = iri_idents[0].text();
    let shape_local = iri_idents[1].text();
    let shape_prefix_iri = lookup_prefix(prefixes, shape_prefix_name)?;
    let shape_iri = SmolStr::from(format!("{shape_prefix_iri}{shape_local}"));

    // Source binding: the EXPR after `from`. For Phase 1's hello.fossil the
    // expression is a bare IDENT primary (`users`), surfacing as
    // `EXPR > LITERAL_EXPR > IDENT`. Recover from the EXPR's first IDENT
    // descendant; falls back to the legacy direct-IDENT shape so any other
    // header form keeps working.
    let source_binding = header
        .children()
        .find(|c| c.kind() == SyntaxKind::EXPR)
        .and_then(|expr_node| {
            expr_node
                .descendants_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::IDENT)
        })
        .map(|t| SmolStr::from(t.text()))?;

    Some(HirMapping {
        name,
        shape_iri,
        source_binding,
    })
}

/// Public-to-the-crate adapter so [`crate::body::body`] can re-use the same
/// `PROPERTY` lowering logic without duplicating it. Per ADR-0005, body
/// content is owned by the `body()` Salsa query, but the per-property
/// shape-and-prefix-aware lowering rules live here next to their natural
/// home (`HirProperty` / `HirExpr`).
///
/// Plan 03-01 Task 2 threads `db` through so the `IRI_EXPR` prefixed-name
/// arm can emit a diagnostic via the Salsa accumulator when the prefix is
/// undeclared (otherwise the property would still be silently dropped — the
/// pre-Phase-3 behaviour the `deferred-items.md` flagged as a bug).
pub(crate) fn lower_property_public(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirProperty> {
    lower_property(db, node, prefixes)
}

fn lower_property(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirProperty> {
    use fossil_syntax::SyntaxKind;

    // PropertyLhs: per Phase 2 grammar.bnf line 141, `PropertyLhs := 'iri' | IRIExpr`.
    // The KW_IRI literal is a direct token child of PROPERTY_LHS; a prefixed
    // name is wrapped in an `IRI_EXPR` sub-node (parser plan 02-03). Walk both
    // forms by collecting all IDENT-or-KW_IRI tokens from descendants.
    let lhs_node = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_LHS)?;
    // Use `descendants_with_tokens` so IRI_EXPR-wrapped IDENT/SHAPE_SEP tokens
    // are still discoverable. Skip trivia.
    let lhs_toks: Vec<_> = lhs_node
        .descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect();

    let key = if lhs_toks.len() == 1
        && (lhs_toks[0].kind() == SyntaxKind::KW_IRI || lhs_toks[0].text() == "iri")
    {
        PropertyKey::Iri
    } else if lhs_toks.len() == 3 && lhs_toks[1].kind() == SyntaxKind::SHAPE_SEP {
        let prefix = lhs_toks[0].text();
        let local = lhs_toks[2].text();
        let prefix_iri = lookup_prefix(prefixes, prefix)?;
        PropertyKey::PrefixedName {
            iri: SmolStr::from(format!("{prefix_iri}{local}")),
        }
    } else {
        return None;
    };

    let expr_node = node.children().find(|c| c.kind() == SyntaxKind::EXPR)?;
    let value = lower_expr(db, &expr_node, prefixes)?;

    Some(HirProperty { key, value })
}

/// Lower an `EXPR` composite node.
///
/// The Phase 1 parser wraps every right-hand side in an `EXPR` whose single
/// child is one of: `TEMPLATE_EXPR`, `LITERAL_EXPR`, `IRI_EXPR`,
/// `FIELD_REF_EXPR`. We dispatch on that inner kind.
///
/// Plan 03-01 Task 2 fix: the `IRI_EXPR` arm now handles BOTH the `ABS_IRI`
/// (`<https://...>`) form AND the `IDENT SHAPE_SEP IDENT` prefixed-name
/// form (e.g. `ex:Foo`). Before this fix, the prefixed-name RHS was silently
/// dropped from `HirBody.properties` (see the `deferred-items.md` from
/// plan 02-06).
fn lower_expr(
    db: &dyn fossil_base::Db,
    expr_node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let inner = expr_node.children().next()?;
    match inner.kind() {
        SyntaxKind::TEMPLATE_EXPR => {
            let tok = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::TEMPLATE)?;
            Some(HirExpr::Template(SmolStr::from(tok.text())))
        }
        SyntaxKind::FIELD_REF_EXPR => {
            // `DOT IDENT` — capture the IDENT.
            let ident = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::IDENT)?;
            Some(HirExpr::FieldRef(SmolStr::from(ident.text())))
        }
        SyntaxKind::LITERAL_EXPR => {
            // Either a `STRING` literal or a bare/prefixed name.
            let toks: Vec<_> = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .filter(|t| {
                    !matches!(
                        t.kind(),
                        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
                    )
                })
                .collect();
            if let Some(s) = toks.iter().find(|t| t.kind() == SyntaxKind::STRING) {
                let raw = s.text();
                let inner_text = raw.trim_start_matches('"').trim_end_matches('"');
                return Some(HirExpr::StringLit(SmolStr::from(inner_text)));
            }
            // `IDENT (SHAPE_SEP IDENT)?` — a bare or prefixed name.
            let idents: Vec<_> = toks
                .iter()
                .filter(|t| t.kind() == SyntaxKind::IDENT)
                .collect();
            if idents.len() == 2 {
                let prefix = idents[0].text();
                let local = idents[1].text();
                let prefix_iri = lookup_prefix(prefixes, prefix)?;
                Some(HirExpr::PrefixedName {
                    iri: SmolStr::from(format!("{prefix_iri}{local}")),
                })
            } else if idents.len() == 1 {
                // Bare identifier — Phase 1 has no other meaning for this so
                // surface it as a FieldRef-shaped expression. Phase 2 grammar
                // distinguishes more precisely (variable vs field vs name).
                Some(HirExpr::FieldRef(SmolStr::from(idents[0].text())))
            } else {
                None
            }
        }
        SyntaxKind::IRI_EXPR => {
            // First, try the ABS_IRI form (`<https://...>`).
            if let Some(abs) = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::ABS_IRI)
            {
                let iri = abs.text().trim_start_matches('<').trim_end_matches('>');
                return Some(HirExpr::PrefixedName {
                    iri: SmolStr::from(iri),
                });
            }

            // Plan 03-01 Task 2 fix: prefixed-name form (`IDENT SHAPE_SEP
            // IDENT`, e.g. `ex:Foo`). Per the parser in
            // `crates/fossil-syntax/src/parser/expr.rs` (lines 285-295), an
            // IRI_EXPR for a prefixed name has exactly three direct token
            // children: IDENT, SHAPE_SEP, IDENT (lexer-contiguous). Reuse
            // the same prefix-table lookup pattern as `lower_property` (LHS).
            let toks: Vec<_> = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .filter(|t| {
                    !matches!(
                        t.kind(),
                        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
                    )
                })
                .collect();
            if toks.len() == 3
                && toks[0].kind() == SyntaxKind::IDENT
                && toks[1].kind() == SyntaxKind::SHAPE_SEP
                && toks[2].kind() == SyntaxKind::IDENT
            {
                let prefix = toks[0].text();
                let local = toks[2].text();
                if let Some(prefix_iri) = lookup_prefix(prefixes, prefix) {
                    return Some(HirExpr::PrefixedName {
                        iri: SmolStr::from(format!("{prefix_iri}{local}")),
                    });
                }
                // Unknown prefix: emit a diagnostic via the Salsa accumulator
                // (rather than the legacy LITERAL_EXPR branch's silent-drop
                // behaviour). The full property still drops via the outer
                // `?`, but at least the user sees WHY. `body()` is a tracked
                // Salsa query so `delay_span_bug`'s accumulator-emit contract
                // holds; the returned `ErrorGuaranteed` is intentionally
                // discarded here (we already convey "drop" via `None`).
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                let _eg = delay_span_bug(
                    db,
                    span,
                    format!("undeclared prefix `{prefix}:` in IRI expression `{prefix}:{local}`"),
                );
                return None;
            }
            None
        }
        _ => None,
    }
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
    fn lower_hello_produces_one_mapping_header() {
        let (db, file) = db_with_hello();
        let hir = lower_to_hir(&db, file);
        let mappings = hir.mappings(&db);
        assert_eq!(mappings.len(), 1);
        let m = &mappings[0];
        assert_eq!(m.name.as_str(), "User");
        assert_eq!(m.shape_iri.as_str(), "https://example.org/Person");
        assert_eq!(m.source_binding.as_str(), "users");
    }

    /// Per ADR-0005, body content (the property list) now lives behind the
    /// `body(db, MappingLoc)` Salsa query. Phase 1's `mappings[0].properties`
    /// access is replaced by `body(db, def_map.mappings()[0]).properties(db)`.
    #[test]
    fn lower_hello_body_has_two_properties() {
        let (db, file) = db_with_hello();
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("hello has one mapping");
        let body = crate::body::body(&db, mloc);
        let props = body.properties(&db);
        assert_eq!(props.len(), 2);
    }

    #[test]
    fn lower_hello_property_zero_is_iri_template() {
        let (db, file) = db_with_hello();
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("hello has one mapping");
        let body = crate::body::body(&db, mloc);
        let p0 = &body.properties(&db)[0];
        assert!(matches!(p0.key, PropertyKey::Iri));
        match &p0.value {
            HirExpr::Template(t) => assert!(t.contains("${.id}"), "template text was {t:?}"),
            other => panic!("expected Template, got {other:?}"),
        }
    }

    #[test]
    fn lower_hello_property_one_is_prefixed_name_field_ref() {
        let (db, file) = db_with_hello();
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("hello has one mapping");
        let body = crate::body::body(&db, mloc);
        let p1 = &body.properties(&db)[1];
        match &p1.key {
            PropertyKey::PrefixedName { iri } => {
                assert_eq!(iri.as_str(), "https://example.org/name");
            }
            PropertyKey::Iri => panic!("expected PrefixedName key, got Iri"),
        }
        match &p1.value {
            HirExpr::FieldRef(f) => assert_eq!(f.as_str(), "name"),
            other => panic!("expected FieldRef value, got {other:?}"),
        }
    }

    #[test]
    fn lower_to_hir_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let a = lower_to_hir(&db, file);
        let b = lower_to_hir(&db, file);
        assert_eq!(a, b);
    }

    // ===== Plan 03-01 Task 2: `IRI_EXPR` prefixed-name arm =====

    /// Fixture that exercises the `IRI_EXPR` prefixed-name RHS form
    /// (`ex:link = ex:Foo`). Pre-plan-03-01 this property was silently
    /// dropped from `HirBody.properties` — see the `deferred-items.md`
    /// under `.planning/phases/02-full-grammar-hir-foundation/`.
    const HELLO_WITH_IRI_RHS: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:link = ex:Foo
";

    /// Same source as [`HELLO_WITH_IRI_RHS`] but with prefix `ex:` REPLACED
    /// by `nope:` on the RHS — so the prefix `nope:` is undeclared. The
    /// LHS keeps `ex:` so the property's key still parses; only the value
    /// fails prefix resolution. Validates the undeclared-prefix diagnostic
    /// path without confounding the test by also breaking the LHS.
    const HELLO_WITH_UNKNOWN_PREFIX_RHS: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:link = nope:Foo
";

    /// Plan 03-01 Task 2 — happy path: `ex:link = ex:Foo` no longer
    /// silently drops. The property appears in `body.properties()` with
    /// a `HirExpr::PrefixedName { iri: "https://example.org/Foo" }` value.
    /// This is the structural fix the deferred-items.md flagged.
    #[test]
    fn iri_expr_lowers_prefixed_name_form() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_WITH_IRI_RHS.to_string(),
            "iri_rhs.fossil".to_string(),
        );
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");
        let body = crate::body::body(&db, mloc);
        let props = body.properties(&db);

        assert_eq!(
            props.len(),
            2,
            "ex:link = ex:Foo must NOT be silently dropped — \
             expected 2 properties (iri + ex:link), got {}: {:?}",
            props.len(),
            props
        );

        // The second property is `ex:link = ex:Foo`.
        let p1 = &props[1];
        match &p1.key {
            PropertyKey::PrefixedName { iri } => {
                assert_eq!(
                    iri.as_str(),
                    "https://example.org/link",
                    "LHS key must resolve via prefix table"
                );
            }
            PropertyKey::Iri => panic!("expected PrefixedName LHS, got Iri"),
        }
        match &p1.value {
            HirExpr::PrefixedName { iri } => {
                assert_eq!(
                    iri.as_str(),
                    "https://example.org/Foo",
                    "RHS prefixed-name must resolve to full IRI via prefix table"
                );
            }
            other => panic!("expected HirExpr::PrefixedName for RHS `ex:Foo`, got {other:?}"),
        }
    }

    /// Plan 03-01 Task 2 — error path: an undeclared prefix on the RHS
    /// (`ex:link = nope:Foo`) emits a diagnostic via the Salsa accumulator
    /// AND still drops the property (matches the rest of `lower_expr`'s
    /// silent-None convention; the `IRI_EXPR` branch is the only one that
    /// adds the diagnostic emit on top).
    #[test]
    fn iri_expr_unknown_prefix_emits_diagnostic() {
        use fossil_base::Diagnostic;

        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_WITH_UNKNOWN_PREFIX_RHS.to_string(),
            "unknown_prefix_rhs.fossil".to_string(),
        );
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        // Drive the body() Salsa query so the accumulator fires.
        let body = crate::body::body(&db, mloc);
        let props = body.properties(&db);
        // `ex:link = nope:Foo` is still dropped (the diagnostic does not
        // prevent the outer property's `?` from short-circuiting). Only
        // the `iri = template` property remains.
        assert_eq!(
            props.len(),
            1,
            "undeclared-prefix RHS still drops the property (silent-None \
             convention), got {} properties",
            props.len()
        );

        // The diagnostic IS emitted via the accumulator, keyed on the
        // body() query that triggered the lowering.
        let diags = crate::body::body::accumulated::<Diagnostic>(&db, mloc);
        assert!(
            !diags.is_empty(),
            "undeclared RHS prefix `nope:` MUST emit at least one Diagnostic \
             (not silent drop)"
        );
        let msg = &diags[0].message;
        assert!(
            msg.contains("nope"),
            "diagnostic must name the offending prefix `nope`, got {msg:?}"
        );
        assert!(
            msg.contains("undeclared") || msg.contains("undefined") || msg.contains("unknown"),
            "diagnostic must say the prefix is undeclared, got {msg:?}"
        );
    }
}
