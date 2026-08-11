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

use fossil_base::{Diagnostic, Severity, SourceFile, Span, delay_span_bug};
use salsa::Accumulator;
use smol_str::SmolStr;

use crate::def_map::{PrefixEntry, def_map};

#[salsa::tracked(debug)]
pub struct HirFile<'db> {
    #[returns(ref)]
    pub mappings: Vec<HirMapping>,
    /// The source bindings whose right-hand side is a PIPELINE rather than an
    /// `io.*` call. A binding that reads a file is `def_map`'s business — a
    /// constructor and a URI, both signature-only; a binding that derives a
    /// relation from another one carries expressions, and expressions are lowered
    /// here or they are lowered twice.
    #[returns(ref)]
    pub source_pipes: Vec<HirSourcePipe>,
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

/// `adultos := users |> where(.edad >= 18)` — a source binding that is a
/// RELATION derived from another binding, not a file to read.
///
/// `base` is the binding at the head of the pipe; every stage after it is one
/// [`HirSourceOp`] in written order. The head must be a name and not another
/// call, because a pipeline whose head is `io.csv("u.csv")` would give the same
/// relation two spellings — and `from <name>` resolves bindings, not expressions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct HirSourcePipe {
    pub name: SmolStr,
    pub base: SmolStr,
    pub ops: Vec<HirSourceOp>,
    /// Start and end of the whole `name := ...` item, so the checker's row
    /// algebra has somewhere to point. One span for the pipeline and not one per
    /// stage: a wrong column is a fact about the pipeline, and per-stage spans
    /// are ADR-0008's side table, not a field.
    pub span: (u32, u32),
}

/// The three verbs of the first version (ADR-0054).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum HirSourceOp {
    /// `where(.edad >= 18)` — keeps the rows the predicate holds for. The row
    /// type is unchanged, which is why it is the cheap one.
    Where(HirExpr),
    /// `select(.id, .nombre)` — restricts the row to the named columns.
    Select(Vec<SmolStr>),
    /// `join(personas, on = .persona_id)` — inner equi-join, and the key is
    /// named ONCE: `on = .k` is `USING (k)`, so `k` must exist on both sides and
    /// appears once in the result. Any other shared name is an error the checker
    /// raises; `.` means *the row* and cannot mean two rows in one expression
    /// while the language has no qualified reference (ADR-0054 §3).
    Join { right: SmolStr, key: SmolStr },
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

/// Comparison and boolean operators — the language's set, defined once.
///
/// It lived in `fossil-mir` until F2 §2, where the HIR needed it: an operator
/// the parser reads and the checker types cannot be defined downstream of both.
/// `fossil-mir` and the backends name this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// One piece of an [`HirExpr::Interpolation`]: literal text, or a hole.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum InterpolationPart {
    /// A literal run, with `{{` already resolved to `{`.
    Text(SmolStr),
    /// A hole. It is an expression like any other, and is typed like one.
    Hole(HirExpr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum HirExpr {
    /// An interpolated string — its literal runs and its holes, in order.
    ///
    /// Both surface spellings arrive here: `"…{u.id}…"` and the backtick
    /// `` `…${.id}…` `` that ADR-0057's seventh amendment retires. A hole holds
    /// an ordinary [`HirExpr`], parsed by the parser, so nothing downstream has
    /// to know a template is made of text.
    ///
    /// It replaced a raw `Template(SmolStr)` that carried the token verbatim
    /// and was scanned again at MIR-lowering time. That is why two walkers used
    /// to be blind to the columns a subject IRI reads: there was no node to
    /// walk. Now there is.
    Interpolation(Vec<InterpolationPart>),
    /// `.id` → field name `"id"`.
    ///
    /// The anonymous row. ADR-0057's ninth amendment ends it: every reference
    /// becomes qualified, and this variant goes with the last fixture that
    /// spells one. It is still here because the surface is being replaced in
    /// stages — see [`Self::ColumnRef`].
    FieldRef(SmolStr),
    /// `orders.user_id` → the `user_id` column of the row `orders` names.
    ///
    /// The qualified reference (ADR-0057, ninth amendment). It shares its CST
    /// shape with a call's callee — `io.csv` is the same `IDENT DOT IDENT` —
    /// so what separates them is the parenthesis, and what separates it from
    /// `clean.slug` (a stdlib function named but not applied) is whether the
    /// head is a catalogued namespace.
    ///
    /// Both spellings are accepted while the surface is replaced in stages.
    /// That is two spellings for one idea, which this house does not keep: the
    /// sequence ends by deleting `FieldRef`, and until it does the language in
    /// this tree is mid-move, not finished.
    ColumnRef { binding: SmolStr, column: SmolStr },
    /// `"hello"` → literal text without surrounding quotes.
    StringLit(SmolStr),
    /// `ex:foo` resolved to its full IRI.
    PrefixedName { iri: SmolStr },
    /// `clean.slug(.name)` — a stdlib function applied to positional arguments.
    ///
    /// `func` is the fully-qualified dotted name exactly as
    /// [`crate::stdlib`] catalogues it (`"clean.slug"`), NOT a backend spelling:
    /// which `DuckDB` builtin or `DataFusion` UDF it becomes is the
    /// materializer's business, resolved from the catalog entry. Arguments are
    /// positional and recursive — `str.concat(clean.trim(.a), "-")` nests.
    Call { func: SmolStr, args: Vec<HirExpr> },
    /// `18` — an integer literal.
    ///
    /// v0.1 carries integers only. A float literal is a diagnostic, not a
    /// silent drop: `Expr` is `Hash + Eq` for Salsa interning and `f64` is
    /// neither, so carrying one needs a decision about its representation
    /// rather than a cast nobody declared.
    IntLit(i64),
    /// `.age >= 18` — a comparison or a boolean connective.
    ///
    /// Arithmetic is NOT here: `+`/`-`/`*`/`/`/`%` parse into the same CST node
    /// and lower to a diagnostic, because MIR has no arithmetic operator to
    /// carry them into. One form at a time, and each one all the way through.
    BinOp {
        op: CmpOp,
        lhs: Box<HirExpr>,
        rhs: Box<HirExpr>,
    },
    /// `.age >= 18 ? "adult" : "minor"` — the conditional.
    ///
    /// Both branches must have the same type and there is no implicit coercion
    /// (`type-system.md` §4.8), which is what makes it a total function of the
    /// row rather than a source of nullable columns.
    Ternary {
        cond: Box<HirExpr>,
        then: Box<HirExpr>,
        otherwise: Box<HirExpr>,
    },
}

#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn lower_to_hir<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> HirFile<'db> {
    let cst = fossil_syntax::parse(db, file);
    let dm = def_map(db, file);
    let prefixes = dm.prefixes(db);

    let mut mappings = Vec::new();
    let mut source_pipes = Vec::new();
    for child in cst.root(db).syntax().children() {
        match child.kind() {
            fossil_syntax::SyntaxKind::MAPPING => {
                if let Some(m) = lower_mapping_node(&child, prefixes) {
                    mappings.push(m);
                }
            }
            fossil_syntax::SyntaxKind::SOURCE_DEF => {
                if let Some(p) = lower_source_pipe(db, &child, prefixes) {
                    source_pipes.push(p);
                }
            }
            _ => {}
        }
    }
    HirFile::new(db, mappings, source_pipes)
}

/// Lower a `SOURCE_DEF` whose right-hand side is a pipeline. Returns `None` for
/// the ordinary `users := io.csv("u.csv")` shape, which carries no expressions
/// and belongs to [`crate::def_map`].
///
/// `PIPELINE_EXPR` nests to the LEFT — `a |> f() |> g()` is `((a |> f()) |> g())`
/// — so the spine is walked down to the head and the stages come back out in
/// written order.
fn lower_source_pipe(
    db: &dyn fossil_base::Db,
    source_def: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirSourcePipe> {
    use fossil_syntax::SyntaxKind;

    let name = source_def
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| SmolStr::from(t.text()))?;

    let rhs = source_def
        .children()
        .find(|c| c.kind() == SyntaxKind::EXPR)?
        .children()
        .next()?;
    if rhs.kind() != SyntaxKind::PIPELINE_EXPR {
        return None;
    }

    let mut stages = Vec::new();
    let mut head = rhs;
    while head.kind() == SyntaxKind::PIPELINE_EXPR {
        let mut parts = head.children();
        let lhs = parts.next()?;
        let stage = parts.next()?;
        stages.push(stage);
        head = lhs;
    }
    stages.reverse();

    let Some(base) = bare_name(&head) else {
        diagnose(
            db,
            &head,
            format!(
                "the head of the source pipeline `{name}` is not a source binding. A source \
                 pipeline starts at a binding and derives from it, e.g. \
                 `{name} := users |> where(.edad >= 18)`."
            ),
        );
        return None;
    };

    let mut ops = Vec::with_capacity(stages.len());
    for stage in &stages {
        ops.push(lower_source_stage(db, stage, prefixes, &name)?);
    }

    let range = source_def.text_range();
    let span = (range.start().into(), range.end().into());
    Some(HirSourcePipe {
        name,
        base,
        ops,
        span,
    })
}

/// One stage of a source pipeline — `where(...)`, `select(...)` or `join(...)`.
fn lower_source_stage(
    db: &dyn fossil_base::Db,
    stage: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
    pipe: &str,
) -> Option<HirSourceOp> {
    use fossil_syntax::SyntaxKind;

    let verb = stage.children().next().as_ref().and_then(bare_name);
    let Some(verb) = verb else {
        diagnose(
            db,
            stage,
            format!("a stage of the source pipeline `{pipe}` is not a verb call."),
        );
        return None;
    };

    // Positional arguments and the `on = ...` named one, kept apart: the verbs
    // read them differently and a positional `on` is not the same word.
    let args: Vec<fossil_syntax::SyntaxNode> = stage
        .children()
        .find(|c| c.kind() == SyntaxKind::ARG_LIST)
        .into_iter()
        .flat_map(|l| l.children())
        .collect();
    let positional: Vec<fossil_syntax::SyntaxNode> = args
        .iter()
        .filter(|a| a.kind() == SyntaxKind::ARG)
        .filter_map(|a| a.children().next())
        .collect();
    let named = |want: &str| -> Option<fossil_syntax::SyntaxNode> {
        args.iter()
            .filter(|a| a.kind() == SyntaxKind::NAMED_ARG)
            .find(|a| {
                a.children_with_tokens()
                    .filter_map(fossil_syntax::SyntaxElement::into_token)
                    .any(|t| t.kind() == SyntaxKind::IDENT && t.text() == want)
            })
            .and_then(|a| a.children().next())
    };

    match verb.as_str() {
        "where" => {
            if positional.len() != 1 {
                diagnose(
                    db,
                    stage,
                    format!(
                        "`where` takes one predicate, and `{pipe}` gives it {}. \
                         e.g. `where(.edad >= 18)`.",
                        positional.len()
                    ),
                );
                return None;
            }
            Some(HirSourceOp::Where(lower_expr_inner(
                db,
                &positional[0],
                prefixes,
            )?))
        }
        "select" => {
            if positional.is_empty() {
                diagnose(
                    db,
                    stage,
                    format!("`select` in `{pipe}` names no column. e.g. `select(.id, .nombre)`."),
                );
                return None;
            }
            let mut cols = Vec::with_capacity(positional.len());
            for arg in &positional {
                let Some(HirExpr::FieldRef(col)) = lower_expr_inner(db, arg, prefixes) else {
                    diagnose(
                        db,
                        arg,
                        format!(
                            "`select` in `{pipe}` takes column references and this is not one. \
                             e.g. `select(.id, .nombre)`."
                        ),
                    );
                    return None;
                };
                cols.push(col);
            }
            Some(HirSourceOp::Select(cols))
        }
        "join" => {
            let right = positional.first().and_then(bare_name);
            let Some(right) = right else {
                diagnose(
                    db,
                    stage,
                    format!(
                        "`join` in `{pipe}` does not name the source binding it joins. \
                         e.g. `join(personas, on = .persona_id)`."
                    ),
                );
                return None;
            };
            // `on = .k` and nothing else: the first version is an equi-join whose
            // key is named once (ADR-0054 §3). An arbitrary condition would have
            // to say which row each `.` names, and that is a qualified reference
            // the language does not have.
            let Some(HirExpr::FieldRef(key)) =
                named("on").and_then(|n| lower_expr_inner(db, &n, prefixes))
            else {
                diagnose(
                    db,
                    stage,
                    format!(
                        "`join` in `{pipe}` needs `on = .<column>`, a column that exists on both \
                         sides and appears once in the result. The first version joins on \
                         equality by name only, so `on = .a == .b` is not it yet."
                    ),
                );
                return None;
            };
            Some(HirSourceOp::Join { right, key })
        }
        other => {
            diagnose(
                db,
                stage,
                format!(
                    "`{other}` is not a source-pipeline verb. The first version has `where`, \
                     `select` and `join`."
                ),
            );
            None
        }
    }
}

/// The text of a node that is exactly one bare `IDENT` — a binding name or a
/// verb. `io.csv` is a dotted callee and deliberately does NOT match.
fn bare_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    use fossil_syntax::SyntaxKind;
    if node.kind() != SyntaxKind::LITERAL_EXPR {
        return None;
    }
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .collect();
    match toks.as_slice() {
        [t] if t.kind() == SyntaxKind::IDENT => Some(SmolStr::from(t.text())),
        _ => None,
    }
}

fn diagnose(db: &dyn fossil_base::Db, node: &fossil_syntax::SyntaxNode, message: String) {
    let range = node.text_range();
    let span = Span::new(range.start().into(), range.end().into());
    Diagnostic::new(Severity::Error, message, span).accumulate(db);
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
    } else if lhs_toks.len() == 1 && lhs_toks[0].kind() == SyntaxKind::AT_ATTR {
        // `@subject(iri = …)` — the same subject, respelled (ADR-0057, seventh
        // amendment §4). Both spellings produce `PropertyKey::Iri`, so nothing
        // below this line knows there are two.
        //
        // Any other `@name` is an error rather than a dropped property: the
        // sigil now parses, and a thing that parses and vanishes is the failure
        // this file's fallback arm exists to prevent.
        if lhs_toks[0].text() != "@subject" {
            let range = lhs_node.text_range();
            let span = Span::new(range.start().into(), range.end().into());
            let name = lhs_toks[0].text();
            Diagnostic::new(
                Severity::Error,
                format!(
                    "`{name}` is not something a mapping body declares. The only \
                     one is `@subject(iri = …)`."
                ),
                span,
            )
            .accumulate(db);
            return None;
        }
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
///
/// # The fallback arm is a diagnostic, not a `None`
///
/// The parser builds fifteen kinds of expression node and this function reads
/// four. Everything else used to reach `_ => None`, and a `None` here means
/// `lower_property` drops the whole property, which `body()` skips without a
/// word — so `ex:slug = clean.slug(.name)` type-checked clean, ran, reported
/// *wrote 1 vertex type*, and emitted a corpus with no `slug` column at all.
/// Measured on 2026-08-06; three programs, two of them silently lossy.
///
/// That is the worst failure mode available to a tool whose stated principle is
/// "if it compiles, it runs", and it happened in the type system's own blind
/// spot: the checker raises nothing because the HIR never saw the expression.
/// The arm now says so. The property is still dropped — closing that needs the
/// eight HIR forms of `decisions/0046-un-nucleo-y-carcasas-finas.md` §2 — but a
/// dropped property is now a red build instead of a quiet hole in the data.
fn lower_expr(
    db: &dyn fossil_base::Db,
    expr_node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirExpr> {
    let inner = expr_node.children().next()?;
    lower_expr_inner(db, &inner, prefixes)
}

/// Lower an expression node that is already unwrapped from its `EXPR` parent.
///
/// `{{` is how a literal `{` is written — the escape Rust, Python and C# share
/// (ADR-0057, seventh amendment §3). The CST keeps the source text verbatim, so
/// resolving it is the HIR's job, and it happens once for every string whether
/// or not that string has a hole.
///
/// **`}}` is NOT an escape here, and that is a departure from the convention
/// the amendment cites.** Rust doubles the closing brace because its format
/// grammar gives `}` meaning wherever it appears; ours gives it meaning only
/// after an opener, so a lone `}` in text has exactly one reading and demanding
/// `}}` would reject text nothing was ambiguous about. If that asymmetry ever
/// surprises someone more than the ceremony would have, this is one line.
fn unescape_braces(text: &str) -> SmolStr {
    if text.contains("{{") {
        SmolStr::from(text.replace("{{", "{"))
    } else {
        SmolStr::from(text)
    }
}

/// Split out from [`lower_expr`] because an argument inside an `ARG_LIST` is an
/// expression in its own right: recursion has to start below the wrapper, not
/// above it.
fn lower_expr_inner(
    db: &dyn fossil_base::Db,
    inner: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let inner = inner.clone();
    match inner.kind() {
        SyntaxKind::POSTFIX_EXPR => lower_postfix(db, &inner, prefixes),
        SyntaxKind::BINARY_EXPR => lower_binary(db, &inner, prefixes),
        SyntaxKind::TERNARY_EXPR => lower_ternary(db, &inner, prefixes),
        SyntaxKind::PIPELINE_EXPR => lower_pipeline(db, &inner, prefixes),
        SyntaxKind::TEMPLATE_EXPR => {
            // A backtick literal the carve left whole: it has no hole, so it is
            // one literal run and nothing else.
            let tok = inner
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .find(|t| t.kind() == SyntaxKind::TEMPLATE)?;
            let text = tok.text().trim_matches('`');
            Some(HirExpr::Interpolation(vec![InterpolationPart::Text(
                unescape_braces(text),
            )]))
        }
        SyntaxKind::INTERP_STRING_EXPR => {
            let mut parts = Vec::new();
            for child in inner.children_with_tokens() {
                match child {
                    fossil_syntax::SyntaxElement::Token(t)
                        if t.kind() == SyntaxKind::STRING_TEXT =>
                    {
                        parts.push(InterpolationPart::Text(unescape_braces(t.text())));
                    }
                    fossil_syntax::SyntaxElement::Node(n)
                        if n.kind() == SyntaxKind::INTERPOLATION =>
                    {
                        // The hole's expression is the one node inside it; the
                        // braces are tokens. A hole that failed to parse leaves
                        // no node, and its diagnostic is already recorded.
                        let hole = n
                            .children()
                            .find_map(|e| lower_expr_inner(db, &e, prefixes))?;
                        parts.push(InterpolationPart::Hole(hole));
                    }
                    _ => {}
                }
            }
            Some(HirExpr::Interpolation(parts))
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
                // A string with no hole still spells `{` as `{{`: an escape
                // whose meaning depended on whether the string happened to
                // contain a hole would be a spelling you have to explain twice.
                return Some(HirExpr::StringLit(unescape_braces(inner_text)));
            }
            if let Some(n) = toks.iter().find(|t| t.kind() == SyntaxKind::INTEGER) {
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                return match n.text().parse::<i64>() {
                    Ok(v) => Some(HirExpr::IntLit(v)),
                    Err(e) => {
                        Diagnostic::new(
                            Severity::Error,
                            format!("`{}` is not an integer fossil can carry: {e}", n.text()),
                            span,
                        )
                        .accumulate(db);
                        None
                    }
                };
            }
            if let Some(f) = toks.iter().find(|t| t.kind() == SyntaxKind::FLOAT) {
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                Diagnostic::new(
                    Severity::Error,
                    format!(
                        "`{}` is a float literal, which fossil cannot carry yet, so this \
                         property will not be written to the corpus. Integer literals work; \
                         a float needs a representation decision the type system has not made.",
                        f.text()
                    ),
                    span,
                )
                .accumulate(db);
                return None;
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
                // Every literal shape this arm knows is handled above. Anything
                // left is a literal the lowering does not read, and a literal it
                // does not read is exactly the silent drop this phase exists to
                // remove: `ex:n = 42` produced no property AND no diagnostic
                // until 2026-08-07.
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                let text = inner.text().to_string();
                Diagnostic::new(
                    Severity::Error,
                    format!(
                        "`{}` is a literal fossil cannot lower yet, so this property will \
                         not be written to the corpus.",
                        text.trim()
                    ),
                    span,
                )
                .accumulate(db);
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
            // `ex:` — a prefix with no local part, which only the parser's
            // interpolation body admits. It resolves to the namespace IRI
            // itself. This is where MIR's `lower_placeholder` used to do the
            // same lookup, against a prefix table MIR had no business holding:
            // the table is threaded through every function in THIS file.
            if toks.len() == 2
                && toks[0].kind() == SyntaxKind::IDENT
                && toks[1].kind() == SyntaxKind::SHAPE_SEP
            {
                let prefix = toks[0].text();
                if let Some(prefix_iri) = lookup_prefix(prefixes, prefix) {
                    return Some(HirExpr::PrefixedName { iri: prefix_iri });
                }
                let range = inner.text_range();
                let span = Span::new(range.start().into(), range.end().into());
                let _eg = delay_span_bug(
                    db,
                    span,
                    format!("undeclared prefix `{prefix}:` in interpolation `{{{prefix}:}}`"),
                );
                return None;
            }
            None
        }
        other => {
            let range = inner.text_range();
            let span = Span::new(range.start().into(), range.end().into());
            let source = inner.text().to_string();
            let source = source.trim();
            Diagnostic::new(
                Severity::Error,
                format!(
                    "`{source}` is not an expression fossil can lower yet, so this property \
                     will not be written to the corpus. A property value may be a template, \
                     a field reference, a string literal, a prefixed name or a call. \
                     (parsed as {other:?})"
                ),
                span,
            )
            .accumulate(db);
            None
        }
    }
}

/// Lower a `PIPELINE_EXPR` — `e |> f(args)`.
///
/// `type-system.md` §4.6: the pipeline passes the LHS as the **first argument**
/// to the RHS function. In expression position that is exactly a call with one
/// more argument, so this is desugaring and not a new HIR form: `.name |>
/// clean.trim()` IS `clean.trim(.name)`, and the checker, the MIR lowering and
/// the backend all see the call they already know.
///
/// The SOURCE-level pipeline — `adultos := users |> where(.edad >= 18)`, which
/// is a relation and not a value — is not this. It is F5 of ADR-0046, and the
/// decision it was waiting on is taken: `join` is in the first version, as an
/// inner equi-join whose key is named once (ADR-0054).
fn lower_pipeline(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirExpr> {
    let range = node.text_range();
    let span = Span::new(range.start().into(), range.end().into());
    let source = node.text().to_string();
    let source = source.trim().to_string();

    let mut parts = node.children();
    let lhs_node = parts.next()?;
    let rhs_node = parts.next()?;
    let lhs = lower_expr_inner(db, &lhs_node, prefixes)?;

    // The RHS is either a call — whose arguments the LHS joins at the front —
    // or a bare function name, which the LHS calls on its own.
    if let Some(HirExpr::Call { func, args }) = lower_expr_inner(db, &rhs_node, prefixes) {
        let mut piped = Vec::with_capacity(args.len() + 1);
        piped.push(lhs);
        piped.extend(args);
        return Some(HirExpr::Call { func, args: piped });
    }

    // `dotted_name` reads the bare-name case without lowering it: a name on its
    // own is not a value (that is the `lower_postfix` diagnostic), but after a
    // pipe it is the function being called.
    if let Some(func) = dotted_name(&rhs_node) {
        return Some(HirExpr::Call {
            func: SmolStr::from(func),
            args: vec![lhs],
        });
    }

    Diagnostic::new(
        Severity::Error,
        format!(
            "the right-hand side of `|>` in `{source}` is not a function. A pipeline passes \
             its left side as the first argument to a stdlib function, e.g. \
             `.name |> clean.trim()`."
        ),
        span,
    )
    .accumulate(db);
    None
}

/// Lower a `TERNARY_EXPR` — `cond ? then : otherwise`.
///
/// The parser leaves three expression children in order and consumes the `?`
/// and the `:` as tokens; a recovery-path ternary (a missing `:`) has an ERROR
/// node among them, and fewer than three expression children means the source
/// is not a ternary the HIR can carry.
fn lower_ternary(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let parts: Vec<_> = node
        .children()
        .filter(|c| c.kind() != SyntaxKind::ERROR)
        .collect();
    if parts.len() != 3 {
        let range = node.text_range();
        let span = Span::new(range.start().into(), range.end().into());
        let source = node.text().to_string();
        Diagnostic::new(
            Severity::Error,
            format!(
                "`{}` is not a complete conditional: it needs a condition, a `?` branch \
                 and a `:` branch.",
                source.trim()
            ),
            span,
        )
        .accumulate(db);
        return None;
    }

    let cond = lower_expr_inner(db, &parts[0], prefixes)?;
    let then = lower_expr_inner(db, &parts[1], prefixes)?;
    let otherwise = lower_expr_inner(db, &parts[2], prefixes)?;
    Some(HirExpr::Ternary {
        cond: Box::new(cond),
        then: Box::new(then),
        otherwise: Box::new(otherwise),
    })
}

/// Lower a `BINARY_EXPR` — a comparison (`.age >= 18`) or a boolean connective
/// (`a and b`).
///
/// Arithmetic parses into this same node and is NOT lowered: MIR has no
/// operator to carry `+` into, and inventing one here would put an expression
/// in the HIR that nothing downstream can execute. It is a diagnostic, which is
/// what the whole phase is about.
fn lower_binary(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let range = node.text_range();
    let span = Span::new(range.start().into(), range.end().into());
    let source = node.text().to_string();
    let source = source.trim().to_string();

    // The operator is the node's own token; the two operands are its child
    // nodes. A malformed binary node (recovery path) has fewer than two.
    let op_token = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::EQ
                    | SyntaxKind::NEQ
                    | SyntaxKind::LT
                    | SyntaxKind::LE
                    | SyntaxKind::GT
                    | SyntaxKind::GE
                    | SyntaxKind::KW_AND
                    | SyntaxKind::KW_OR
                    | SyntaxKind::PLUS
                    | SyntaxKind::MINUS
                    | SyntaxKind::STAR
                    | SyntaxKind::SLASH
                    | SyntaxKind::PERCENT
            )
        })?;

    let op = match op_token.kind() {
        SyntaxKind::EQ => CmpOp::Eq,
        SyntaxKind::NEQ => CmpOp::Ne,
        SyntaxKind::LT => CmpOp::Lt,
        SyntaxKind::LE => CmpOp::Le,
        SyntaxKind::GT => CmpOp::Gt,
        SyntaxKind::GE => CmpOp::Ge,
        SyntaxKind::KW_AND => CmpOp::And,
        SyntaxKind::KW_OR => CmpOp::Or,
        _ => {
            Diagnostic::new(
                Severity::Error,
                format!(
                    "`{source}` is arithmetic, which fossil cannot lower yet, so this \
                     property will not be written to the corpus. Comparisons (`==`, `!=`, \
                     `<`, `<=`, `>`, `>=`) and `and`/`or` work; `{}` does not.",
                    op_token.text()
                ),
                span,
            )
            .accumulate(db);
            return None;
        }
    };

    let mut operands = node.children();
    let lhs_node = operands.next()?;
    let rhs_node = operands.next()?;
    let lhs = lower_expr_inner(db, &lhs_node, prefixes)?;
    let rhs = lower_expr_inner(db, &rhs_node, prefixes)?;

    Some(HirExpr::BinOp {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    })
}

/// Lower a `POSTFIX_EXPR` — either a call (`clean.slug(.name)`) or the member
/// access that names its callee (`clean.slug`).
///
/// The parser builds both with the same node kind, left-associatively: the call
/// node carries an `LPAREN` token and wraps the member-access node, which in
/// turn wraps the base `LITERAL_EXPR`. So "is this a call?" is "does this node
/// hold an `LPAREN`", and the callee's dotted name is read off the chain below.
fn lower_postfix(
    db: &dyn fossil_base::Db,
    node: &fossil_syntax::SyntaxNode,
    prefixes: &[PrefixEntry],
) -> Option<HirExpr> {
    use fossil_syntax::SyntaxKind;

    let range = node.text_range();
    let span = Span::new(range.start().into(), range.end().into());
    let source = node.text().to_string();
    let source = source.trim().to_string();

    let is_call = node
        .children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .any(|t| t.kind() == SyntaxKind::LPAREN);

    if !is_call {
        // `orders.user_id` — a qualified column reference (ADR-0057, ninth
        // amendment). It reaches here because the CST cannot tell it from a
        // call's callee: both are `IDENT DOT IDENT`. The parenthesis separates
        // those two, and the stdlib catalogue separates this from the case
        // below: `clean` is a namespace, `orders` is not.
        if let Some(dotted) = dotted_name(node) {
            let mut parts = dotted.split('.');
            if let (Some(head), Some(column), None) = (parts.next(), parts.next(), parts.next())
                && crate::stdlib::stdlib()
                    .iter()
                    .all(|e| !e.name.starts_with(&format!("{head}.")))
            {
                return Some(HirExpr::ColumnRef {
                    binding: SmolStr::from(head),
                    column: SmolStr::from(column),
                });
            }
        }

        // `ex:name = clean.slug` — a function named but never applied. v0.1 has
        // no function values (ADR-0046 §2 takes partial application out of the
        // grammar), so this is an error, not a value that quietly becomes text.
        Diagnostic::new(
            Severity::Error,
            format!(
                "`{source}` names a function but does not call it. Fossil has no function \
                 values: write `{source}(...)` with its arguments."
            ),
            span,
        )
        .accumulate(db);
        return None;
    }

    let callee = node.children().next()?;
    let Some(func) = dotted_name(&callee) else {
        Diagnostic::new(
            Severity::Error,
            format!(
                "`{source}` calls something that is not a stdlib function name. Only a \
                 catalogued name may be called, e.g. `clean.trim(.name)`."
            ),
            span,
        )
        .accumulate(db);
        return None;
    };

    let mut args = Vec::new();
    if let Some(list) = node.children().find(|c| c.kind() == SyntaxKind::ARG_LIST) {
        for arg in list.children() {
            if arg.kind() == SyntaxKind::NAMED_ARG {
                let r = arg.text_range();
                Diagnostic::new(
                    Severity::Error,
                    format!(
                        "`{}` passes a named argument to `{func}`. v0.1 arguments are \
                         positional.",
                        arg.text().to_string().trim()
                    ),
                    Span::new(r.start().into(), r.end().into()),
                )
                .accumulate(db);
                return None;
            }
            let inner = arg.children().next()?;
            // An argument that does not lower has already said why (the arm
            // above accumulates); dropping the whole call keeps the property
            // from being written with a hole in it.
            args.push(lower_expr_inner(db, &inner, prefixes)?);
        }
    }

    Some(HirExpr::Call {
        func: SmolStr::from(func),
        args,
    })
}

/// The dotted name a callee chain spells: `clean.slug` → `"clean.slug"`.
///
/// Returns `None` for anything that is not a plain name — `f(x).y`, a template,
/// a field reference. The catalog is keyed by these strings, so a callee that
/// cannot produce one cannot be looked up.
fn dotted_name(node: &fossil_syntax::SyntaxNode) -> Option<String> {
    use fossil_syntax::SyntaxKind;

    let idents = |n: &fossil_syntax::SyntaxNode| -> Vec<String> {
        n.children_with_tokens()
            .filter_map(fossil_syntax::SyntaxElement::into_token)
            .filter(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| t.text().to_string())
            .collect()
    };

    match node.kind() {
        SyntaxKind::LITERAL_EXPR => {
            let ids = idents(node);
            (ids.len() == 1).then(|| ids[0].clone())
        }
        SyntaxKind::POSTFIX_EXPR => {
            // A member access: the base chain, then this node's own IDENT.
            let has_paren = node
                .children_with_tokens()
                .filter_map(fossil_syntax::SyntaxElement::into_token)
                .any(|t| t.kind() == SyntaxKind::LPAREN);
            if has_paren {
                return None; // `f(x).y` — the base is a call, not a name
            }
            let base = dotted_name(&node.children().next()?)?;
            let ids = idents(node);
            (ids.len() == 1).then(|| format!("{base}.{}", ids[0]))
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

    fn lower_src(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    /// `users.name` lowers to a qualified column reference (ADR-0057, ninth
    /// amendment). It arrives at the parser as `IDENT DOT IDENT` — the very
    /// shape of a call's callee — so this pins the branch that separates them.
    #[test]
    fn a_qualified_reference_lowers_to_a_column_ref() {
        let (db, file) = lower_src(
            "prefix ex: <https://example.org/>\n\nusers := io.csv(\"u.csv\")\n\nUser : ex:Person from users\n    ex:name = users.name\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        assert_eq!(
            body.properties(&db)[0].value,
            HirExpr::ColumnRef {
                binding: "users".into(),
                column: "name".into()
            }
        );
    }

    /// And the diagnostic it shares a CST shape with SURVIVES: `clean.slug` is
    /// a catalogued namespace, so it is still a function named but not applied,
    /// not a column of a row called `clean`. The stdlib catalogue is what tells
    /// the two apart.
    #[test]
    fn a_stdlib_name_without_its_call_is_still_an_error() {
        let (db, file) = lower_src(
            "prefix ex: <https://example.org/>\n\nusers := io.csv(\"u.csv\")\n\nUser : ex:Person from users\n    ex:name = clean.slug\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        assert!(
            body.properties(&db).is_empty(),
            "a function named but not called must not lower to a value"
        );
    }

    /// The spelling the seventh amendment replaces the backtick with: a plain
    /// string, interpolated with `{expr}`, resolved at compile time.
    ///
    /// The IRI is written in full — no `base`, no CURIE with holes — and the
    /// hole is a QUALIFIED reference, which is only expressible because the
    /// hole is parsed by the expression parser. The old `${…}` scan understood
    /// exactly two shapes and echoed anything else back as text.
    #[test]
    fn a_quoted_string_interpolates_and_its_hole_is_an_expression() {
        let (db, file) = lower_src(
            "prefix ex: <https://example.org/>\n\nusers := io.csv(\"u.csv\")\n\nUser : ex:Person from users\n    iri = \"https://example.org/user/{users.id}\"\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        let HirExpr::Interpolation(parts) = &body.properties(&db)[0].value else {
            panic!(
                "expected an interpolation, got {:?}",
                body.properties(&db)[0].value
            );
        };
        assert_eq!(
            parts,
            &vec![
                InterpolationPart::Text("https://example.org/user/".into()),
                InterpolationPart::Hole(HirExpr::ColumnRef {
                    binding: "users".into(),
                    column: "id".into(),
                }),
            ]
        );
    }

    /// `{{` is a literal brace whether or not the string has a hole — an escape
    /// whose meaning depended on that would be a spelling you explain twice
    /// (house rule 2) — and `}` needs no escape at all, because outside a hole
    /// it has only one reading. See `unescape_braces` for why that asymmetry is
    /// deliberate rather than half a convention copied badly.
    #[test]
    fn a_doubled_brace_is_one_brace_and_a_closing_brace_needs_no_escape() {
        let (db, file) = lower_src(
            "prefix ex: <https://example.org/>\n\nusers := io.csv(\"u.csv\")\n\nUser : ex:Person from users\n    ex:a = \"{{literal}\"\n    ex:b = \"{{x}{users.id}\"\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        let props = body.properties(&db);
        assert_eq!(
            props[0].value,
            HirExpr::StringLit("{literal}".into()),
            "a string with no hole still resolves `{{{{`, and keeps a lone `}}`"
        );
        let HirExpr::Interpolation(parts) = &props[1].value else {
            panic!("expected an interpolation, got {:?}", props[1].value);
        };
        assert_eq!(
            parts.first(),
            Some(&InterpolationPart::Text("{x}".into())),
            "and so does one that has a hole after it"
        );
    }

    /// `@subject(iri = …)` and `iri = …` are the same subject, and they must
    /// lower to the same thing — otherwise the fixture rewrite in the next
    /// commit would change meaning while it changes spelling.
    #[test]
    fn the_two_subject_spellings_lower_identically() {
        const HEAD: &str = "prefix ex: <https://example.org/>\n\nusers := io.csv(\"u.csv\")\n\nUser : ex:Person from users\n";
        let (db_old, old) = lower_src(&format!(
            "{HEAD}    iri = \"https://example.org/u/{{users.id}}\"\n"
        ));
        let (db_new, new) = lower_src(&format!(
            "{HEAD}    @subject(iri = \"https://example.org/u/{{users.id}}\")\n"
        ));
        let props_of = |db: &fossil_base::FossilDb, file| {
            let m = crate::def_map::def_map(db, file).mappings(db)[0];
            crate::body::body(db, m).properties(db).clone()
        };
        let old_props = props_of(&db_old, old);
        let new_props = props_of(&db_new, new);
        assert!(
            matches!(new_props[0].key, PropertyKey::Iri),
            "`@subject` is the subject key, got {:?}",
            new_props[0].key
        );
        assert_eq!(
            old_props, new_props,
            "the sigil is a spelling, not a different property"
        );
    }

    /// The sigil now parses, so an unknown one must be an ERROR and not a
    /// property that quietly disappears — the failure this file's fallback arm
    /// exists to prevent.
    #[test]
    fn an_unknown_attribute_is_a_diagnostic_and_not_a_dropped_property() {
        let (db, file) = lower_src(
            "prefix ex: <https://example.org/>\n\nusers := io.csv(\"u.csv\")\n\nUser : ex:Person from users\n    @sensitive(iri = \"x\")\n    ex:name = users.name\n",
        );
        let mapping = crate::def_map::def_map(&db, file).mappings(&db)[0];
        let body = crate::body::body(&db, mapping);
        assert_eq!(
            body.properties(&db).len(),
            1,
            "the good property survives; only the bad attribute drops"
        );
        let diags = crate::body::body::accumulated::<Diagnostic>(&db, mapping);
        assert!(
            diags.iter().any(|d| d.message.contains("@sensitive")),
            "the diagnostic must name the attribute, got {diags:?}"
        );
    }

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

    /// The call that used to be a hole is now a `Call` in the HIR.
    ///
    /// Until 2026-08-07 this exact program was the regression fixture for a
    /// measured data-loss bug: `ex:slug = clean.slug(.name)` was dropped from
    /// `HirBody` in silence, `fossil check` said *ok*, `fossil run` said *wrote
    /// 1 vertex type*, and the column was absent from the corpus. Then the drop
    /// was made loud. This is the same program with the hole closed: the
    /// property survives lowering, carrying the function's name and its
    /// argument, and no diagnostic is raised at all.
    #[test]
    fn a_call_lowers_to_a_call_and_raises_nothing() {
        const CALLS_A_BUILTIN: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:slug = clean.slug(.name)
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, CALLS_A_BUILTIN.to_string(), "t.fossil".to_string());

        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        let diagnostics = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diagnostics.is_empty(),
            "a call the lowering understands must raise nothing, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );

        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 2, "both properties survive lowering");
        let HirExpr::Call { func, args } = &props[1].value else {
            panic!("expected a Call, got {:?}", props[1].value);
        };
        assert_eq!(func.as_str(), "clean.slug");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], HirExpr::FieldRef(SmolStr::from("name")));
    }

    /// A comparison lowers, with its integer literal.
    ///
    /// The literal is half the point: until 2026-08-07 `ex:n = 42` was dropped
    /// **with no diagnostic at all** — the 2026-08-06 fix made unknown node
    /// KINDS loud, and a `LITERAL_EXPR` holding an integer is a known kind
    /// whose arm returned `None`. A second silent hole in the same blind spot,
    /// found by needing a right-hand side for this test.
    #[test]
    fn a_comparison_lowers_with_its_integer_literal() {
        const COMPARES: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:adult = .age >= 18
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, COMPARES.to_string(), "t.fossil".to_string());
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        let diagnostics = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diagnostics.is_empty(),
            "a comparison the lowering understands must raise nothing, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 2);
        let HirExpr::BinOp { op, lhs, rhs } = &props[1].value else {
            panic!("expected a BinOp, got {:?}", props[1].value);
        };
        assert_eq!(*op, CmpOp::Ge);
        assert_eq!(**lhs, HirExpr::FieldRef(SmolStr::from("age")));
        assert_eq!(**rhs, HirExpr::IntLit(18));
    }

    /// The loud-drop guarantee, re-pinned on a form that is still a hole.
    ///
    /// `call` (F2 §1) and `comparison` (F2 §2) have landed; `conditional` and
    /// `pipeline` follow, and arithmetic has no MIR operator to be carried
    /// into. Until then a property whose value is one of them is still dropped
    /// — and this asserts the drop stays **loud**, which is the difference
    /// between a known limitation and silent corruption. The day there is no
    /// form left, this test is deleted, not weakened.
    #[test]
    fn an_unlowerable_expression_is_a_diagnostic_and_not_a_silent_drop() {
        const ARITHMETIC: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:doble = .id * 2
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, ARITHMETIC.to_string(), "t.fossil".to_string());

        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        let diagnostics = crate::body::body::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            !diagnostics.is_empty(),
            "an expression the lowering cannot read must produce a diagnostic",
        );
        let d = &diagnostics[0];
        assert_eq!(d.severity, fossil_base::Severity::Error);
        assert!(
            d.message.contains(".id * 2"),
            "the diagnostic must quote what the user wrote, got: {}",
            d.message,
        );
        assert!(
            d.span.end > d.span.start,
            "the diagnostic must point somewhere, got {:?}",
            d.span,
        );

        let props = crate::body::body(&db, mloc).properties(&db);
        assert_eq!(props.len(), 1, "the unlowerable property is still dropped");
    }

    /// A name that is not catalogued is a type error, not a lowering hole: the
    /// HIR carries the call, and the checker is what refuses it.
    #[test]
    fn an_uncatalogued_function_lowers_and_the_checker_refuses_it() {
        const UNKNOWN_FN: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:slug = clean.sluggify(.name)
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, UNKNOWN_FN.to_string(), "t.fossil".to_string());
        let dm = crate::def_map::def_map(&db, file);
        let mloc = *dm.mappings(&db).first().expect("one mapping");

        assert_eq!(
            crate::body::body(&db, mloc).properties(&db).len(),
            2,
            "lowering carries the call; resolving the name is the checker's job"
        );
        let diags =
            crate::check::typecheck_mapping::accumulated::<fossil_base::Diagnostic>(&db, mloc);
        assert!(
            diags.iter().any(|d| d.message.contains("clean.sluggify")
                && d.message.contains("did you mean")),
            "the checker must name the function and suggest one, got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
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
        // The subject is parts now, not text: the prefix resolved to its IRI at
        // lowering time and the hole is a `FieldRef` node, so this asserts on
        // the tree rather than on a substring of the token.
        match &p0.value {
            HirExpr::Interpolation(parts) => {
                assert_eq!(
                    parts.first(),
                    Some(&InterpolationPart::Hole(HirExpr::PrefixedName {
                        iri: "https://example.org/".into()
                    })),
                    "the prefix hole resolves to the prefix IRI, in HIR"
                );
                assert!(
                    parts.iter().any(|p| matches!(
                        p,
                        InterpolationPart::Hole(HirExpr::FieldRef(f)) if f == "id"
                    )),
                    "the field hole is a field reference, got {parts:?}"
                );
            }
            other => panic!("expected an interpolation, got {other:?}"),
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

    fn db_with(src: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "t.fossil".to_string());
        (db, file)
    }

    /// A source pipeline lowers to its verbs, in the order they were written,
    /// and a binding that reads a file does not become one.
    ///
    /// `PIPELINE_EXPR` nests to the left, so this is also the test that the
    /// spine is walked the right way round: `join` then `where`, not the
    /// reverse.
    #[test]
    fn a_source_pipeline_lowers_to_its_verbs_in_written_order() {
        const PIPES: &str = "\
users := io.csv(\"u.csv\")
personas := io.csv(\"p.csv\")
adultos := users |> where(.edad >= 18)
breve := adultos |> select(.id, .nombre)
ventas := adultos |> join(personas, on = .persona_id) |> where(.total >= 100)
";
        let (db, file) = db_with(PIPES);
        let hir = lower_to_hir(&db, file);
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        assert!(
            diags.is_empty(),
            "the three verbs of the first version raise nothing, got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );

        let pipes = hir.source_pipes(&db);
        // Two `io.csv` bindings are NOT pipelines: they carry no expression.
        assert_eq!(
            pipes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["adultos", "breve", "ventas"],
        );

        assert_eq!(pipes[0].base.as_str(), "users");
        assert!(matches!(pipes[0].ops.as_slice(), [HirSourceOp::Where(_)]));

        assert_eq!(pipes[1].base.as_str(), "adultos");
        let [HirSourceOp::Select(cols)] = pipes[1].ops.as_slice() else {
            panic!("expected one Select, got {:?}", pipes[1].ops);
        };
        assert_eq!(
            cols.iter().map(SmolStr::as_str).collect::<Vec<_>>(),
            ["id", "nombre"]
        );

        assert_eq!(pipes[2].base.as_str(), "adultos");
        let [HirSourceOp::Join { right, key }, HirSourceOp::Where(pred)] = pipes[2].ops.as_slice()
        else {
            panic!("expected Join then Where, got {:?}", pipes[2].ops);
        };
        assert_eq!(right.as_str(), "personas");
        assert_eq!(key.as_str(), "persona_id");
        assert!(matches!(pred, HirExpr::BinOp { op: CmpOp::Ge, .. }));
    }

    /// `on = .a == .b` is refused, and it is refused by name.
    ///
    /// ADR-0054 §3: the first version joins on equality by name, so the
    /// condition is a column and not a comparison. The point of the test is that
    /// the pipeline does not lower — a join whose key we guessed would produce a
    /// corpus nobody asked for, which is the failure this project exists to make
    /// impossible.
    #[test]
    fn an_arbitrary_join_condition_is_a_diagnostic_and_not_a_join() {
        const ARBITRARY: &str = "\
pedidos := io.csv(\"o.csv\")
personas := io.csv(\"p.csv\")
ventas := pedidos |> join(personas, on = .persona_id == .id)
";
        let (db, file) = db_with(ARBITRARY);
        let hir = lower_to_hir(&db, file);
        assert!(
            hir.source_pipes(&db).is_empty(),
            "the pipeline must not lower, got {:?}",
            hir.source_pipes(&db),
        );
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        let msg = diags.first().map(|d| d.message.clone()).unwrap_or_default();
        assert!(
            msg.contains("on = .<column>"),
            "the diagnostic must show the form that works, got {msg:?}"
        );
    }

    /// A verb the first version does not have is named, not ignored.
    #[test]
    fn an_unknown_source_verb_is_a_diagnostic() {
        const UNKNOWN: &str = "\
users := io.csv(\"u.csv\")
raro := users |> group_by(.edad)
";
        let (db, file) = db_with(UNKNOWN);
        let hir = lower_to_hir(&db, file);
        assert!(hir.source_pipes(&db).is_empty());
        let diags = lower_to_hir::accumulated::<Diagnostic>(&db, file);
        let msg = diags.first().map(|d| d.message.clone()).unwrap_or_default();
        assert!(
            msg.contains("group_by") && msg.contains("where"),
            "the diagnostic must name the verb and the ones that exist, got {msg:?}"
        );
    }
}
