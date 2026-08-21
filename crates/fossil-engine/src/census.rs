//! What the compiler UNDERSTOOD from a program — the reading, not the verdict.
//!
//! [`crate::check`] answers «is anything wrong», and a program can be entirely
//! right about nothing. The hole this module was built to close: a property
//! whose right-hand side the lowering cannot read is dropped in silence.
//! [`fossil_hir::body::body`] walks the `PROPERTY` children of a mapping body
//! and keeps the ones `lower_property` returns `Some` for; the `None` arm is a
//! bare `if let`, with no `else`, so a mapping that wrote five properties and
//! lowered two produces a graph missing three columns.
//!
//! # That premise is no longer true, and it matters for where this belongs
//!
//! **Every `None` this module was built to catch now carries a diagnostic.**
//! `lower_property`'s four refusal paths each `diagnose` before returning, and
//! `lower_expr`'s catch-all — the one that actually caused it, measured on
//! 2026-08-06 across three programs, two of them silently lossy — was turned
//! from `_ => None` into a diagnostic then. `body.rs` is still a bare `if let`,
//! but the `None` arriving there has already been announced. So `check` DOES
//! now say what this module was written because it could not say, and since the
//! drain moved to [`fossil_mir::program_diagnostics`] it says it in the editor
//! and the browser too.
//!
//! What is left is not a capability, it is a **cross-check**: proof, per
//! program, that written == lowered, which would catch a FUTURE silent arm the
//! way nothing caught the last one. That is conformance-harness machinery, and
//! its only consumer is `tests/programs.rs`. It sits in a production crate,
//! behind a native tripwire, reachable from nothing a user can run — 327 lines
//! of `pub` surface with no caller and no embedding.
//!
//! **Where it goes, if it moves.** The counting is a fact about `fossil-hir`'s
//! own CST↔`HirBody` relation and could live there; [`ProgramCensus::render`]
//! is the harness's committed-artefact format and could only live beside the
//! harness. Neither move is blocked by anything here — both destinations are
//! held by other work in flight.
//!
//! So the census is two counts and their difference, per mapping: the
//! properties the AUTHOR WROTE (`PROPERTY` nodes in the CST) against the
//! properties that SURVIVED (`HirBody::properties`). Equality is the invariant
//! — a compiler may reject a property, but it may not lose one.
//!
//! It runs over the same [`crate::system::open_db`] as `check` and `run`, which
//! is the point: the shape documents the program names are registered, the real
//! `ShEx` decoder is installed, and the reading reported here is the reading the
//! executor will act on. A second database built beside this one would be a
//! second compiler.

use std::fmt::Write as _;
use std::path::Path;

use fossil_hir::body::{body, mapping_cst_node};
use fossil_hir::def_map::{ShapeBindError, def_map};
use fossil_hir::display::pipe_text;
use fossil_hir::lower::{PropertyKey, lower_to_hir};
use fossil_syntax::SyntaxKind;

use crate::system::open_db;

/// One `type { … } := io.shex("…")` name, and what it bound.
#[derive(Debug, Clone)]
pub struct TypeBinding {
    pub name: String,
    /// The shape IRI the name resolved to, or `None` when it bound nothing.
    pub shape_iri: Option<String>,
    /// Why it bound nothing — never a bare `None` when `shape_iri` is `None`.
    pub error: Option<ShapeBindError>,
    /// The document the constructor named.
    pub document: Option<String>,
}

/// One `Name := …` binding, and what the reading made of its right-hand side.
#[derive(Debug, Clone)]
pub struct SourceBinding {
    pub name: String,
    pub rhs: SourceRhs,
}

/// The two shapes a source binding's right-hand side has.
///
/// They were one — a constructor and a URI, read by the same token scan — and a
/// pipeline has no URI, so every derived binding rendered as `LineRow.join(?)`.
/// That is what kept `compound-key` from being blessed: its whole claim is the
/// two-column key, and the artefact could not say what the key was. Dropping the
/// `tenant` conjunct takes the join from four rows to seven and the seven mint
/// the same four subjects, so the vertex count is equal either way.
#[derive(Debug, Clone)]
pub enum SourceRhs {
    /// `io.csv("data/orders.csv")` — a file to read. Both halves are the
    /// signature-only reading `def_map` performs, and `None` is the reading
    /// failing rather than the program omitting something.
    Read {
        /// `io.csv` / `io.json` / … — `None` when the RHS is not a recognisable call.
        constructor: Option<String>,
        /// The first positional string argument.
        uri: Option<String>,
    },
    /// `LineRow.join(OrderRow, on = …)` — a relation derived from another
    /// binding, rendered from the HIR by [`fossil_hir::display::pipe_text`].
    /// Rendering it from the source text would compare the artefact against its
    /// own input.
    Derive {
        /// The pipeline: the base binding and every stage, in written order.
        pipe: String,
        /// Written in a mapping header (`Orders : Order from Purchase.join(…)`)
        /// rather than bound to a name of its own.
        ///
        /// The relation is anonymous there and `lower_to_hir` registers it under
        /// the MAPPING's name, which is why it is in `source_pipes` and not in
        /// `def_map`'s bindings. The census listed the bindings alone, so the
        /// join of `shop` — the one conformance program with an edge — was
        /// absent from its own artefact.
        inline: bool,
    },
}

/// One mapping's reading: what its body said, and what survived saying it.
#[derive(Debug, Clone)]
pub struct MappingCensus {
    /// The mapping's own name, off its header (`Users` in `Users : Person from …`).
    pub name: String,
    /// The left-hand side of every `PROPERTY` node the parser produced, in
    /// source order — `@subject`, `name`, `email`. Read as the text before the
    /// first `=`, deliberately: it is what the AUTHOR wrote, and reading it
    /// through the parser's own node shape would make the census agree with the
    /// parser about a property whose LHS the parser mis-shaped.
    pub written: Vec<String>,
    /// The file-absolute byte range of each written property, parallel to
    /// [`Self::written`].
    ///
    /// Carried so a caller can ask the question that separates the two ways a
    /// property is lost: was anything SAID about it? `lower_binary` refuses
    /// arithmetic with a diagnostic and then returns `None`; `lower_property`
    /// refuses a form it cannot read by returning `None` and nothing else. Both
    /// end with the property missing from the corpus, and only one of them tells
    /// the author. Without the span the two are indistinguishable from here.
    pub spans: Vec<(u32, u32)>,
    /// The key of every property that reached [`fossil_hir::body::HirBody`].
    pub lowered: Vec<String>,
}

impl MappingCensus {
    /// Indices into [`Self::written`] that no lowered property accounts for —
    /// order-preserving multiset difference. Empty is the invariant.
    #[must_use]
    pub fn dropped_indices(&self) -> Vec<usize> {
        let mut remaining = self.lowered.clone();
        let mut out = Vec::new();
        for (i, key) in self.written.iter().enumerate() {
            if let Some(j) = remaining.iter().position(|k| k == key) {
                remaining.remove(j);
            } else {
                out.push(i);
            }
        }
        out
    }

    /// The written keys that were lost.
    #[must_use]
    pub fn dropped(&self) -> Vec<String> {
        self.dropped_indices()
            .into_iter()
            .map(|i| self.written[i].clone())
            .collect()
    }
}

/// A whole program's reading.
#[derive(Debug, Clone)]
pub struct ProgramCensus {
    pub types: Vec<TypeBinding>,
    pub sources: Vec<SourceBinding>,
    pub mappings: Vec<MappingCensus>,
}

impl ProgramCensus {
    /// Every `PROPERTY` node the file produced, over every mapping.
    #[must_use]
    pub fn written(&self) -> usize {
        self.mappings.iter().map(|m| m.written.len()).sum()
    }

    /// Every property that survived lowering, over every mapping.
    #[must_use]
    pub fn lowered(&self) -> usize {
        self.mappings.iter().map(|m| m.lowered.len()).sum()
    }

    /// `(mapping name, dropped key, its file-absolute span)` for every property
    /// the lowering lost. Whether the loss was ANNOUNCED is the caller's
    /// question to ask, by looking for a diagnostic over the span.
    #[must_use]
    pub fn drops(&self) -> Vec<(String, String, (u32, u32))> {
        self.mappings
            .iter()
            .flat_map(|m| {
                m.dropped_indices()
                    .into_iter()
                    .map(|i| (m.name.clone(), m.written[i].clone(), m.spans[i]))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The census as the stable text an artefact is compared byte-for-byte.
    ///
    /// Every number in it is derived, nothing is a constant shared with the
    /// compiler, and the layout is line-per-fact so a diff names the fact that
    /// changed rather than a reflowed paragraph.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("── type bindings ──\n");
        if self.types.is_empty() {
            out.push_str("(none)\n");
        }
        for t in &self.types {
            let _ = write!(out, "{} ← {}", t.name, t.document.as_deref().unwrap_or("?"));
            match (&t.shape_iri, &t.error) {
                (Some(iri), _) => {
                    let _ = writeln!(out, " = {iri}");
                }
                (None, Some(e)) => {
                    let _ = writeln!(out, " BOUND NOTHING: {e:?}");
                }
                (None, None) => out.push_str(" BOUND NOTHING, and said no why\n"),
            }
        }

        out.push_str("\n── source bindings ──\n");
        if self.sources.is_empty() {
            out.push_str("(none)\n");
        }
        for s in &self.sources {
            match &s.rhs {
                SourceRhs::Read { constructor, uri } => {
                    let _ = writeln!(
                        out,
                        "{} := {}({})",
                        s.name,
                        constructor.as_deref().unwrap_or("?"),
                        uri.as_deref().unwrap_or("?"),
                    );
                }
                // `from` and not `:=` for an inline one, because that is the
                // binder the program wrote. Spelling it `:=` would put a
                // binding in the artefact that the author never made.
                SourceRhs::Derive { pipe, inline } => {
                    let binder = if *inline { "from" } else { ":=" };
                    let _ = writeln!(out, "{} {binder} {pipe}", s.name);
                }
            }
        }

        out.push_str("\n── mappings ──\n");
        if self.mappings.is_empty() {
            out.push_str("(none)\n");
        }
        for m in &self.mappings {
            let _ = writeln!(
                out,
                "{}: {} written, {} lowered",
                m.name,
                m.written.len(),
                m.lowered.len()
            );
            let mut remaining = m.lowered.clone();
            for key in &m.written {
                if let Some(i) = remaining.iter().position(|k| k == key) {
                    remaining.remove(i);
                    let _ = writeln!(out, "    {key}");
                } else {
                    let _ = writeln!(out, "    {key}  ← DROPPED IN SILENCE");
                }
            }
            for key in remaining {
                let _ = writeln!(out, "    {key}  ← lowered from no PROPERTY node");
            }
        }
        out
    }
}

/// Read `path` through the production compile path and report what it understood.
///
/// # Errors
/// Returns a read error if `path` is unreadable.
pub fn census(path: &Path) -> miette::Result<ProgramCensus> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let (db, file) = open_db(text, path);
    let dm = def_map(&db, file);

    let types = dm
        .types(&db)
        .iter()
        .map(|t| TypeBinding {
            name: t.name.to_string(),
            shape_iri: t.shape_iri.as_ref().map(ToString::to_string),
            error: t.shape_error.clone(),
            document: t.document.as_ref().map(ToString::to_string),
        })
        .collect();

    // A binding that derives a relation is a PIPELINE, and pipelines are lowered
    // in `fossil-hir`'s `lower`, not read off the `SOURCE_DEF` header the way a
    // constructor and a URI are. So the census asks both and the name is what
    // joins them: `def_map` holds every binding in written order, `source_pipes`
    // holds the derived ones, and a binding in both is a derived one.
    let hir = lower_to_hir(&db, file);
    let pipes = hir.source_pipes(&db);
    let mut sources: Vec<SourceBinding> = dm
        .sources(&db)
        .iter()
        .map(|s| SourceBinding {
            name: s.name.to_string(),
            rhs: pipes.iter().find(|p| p.name == s.name).map_or_else(
                || SourceRhs::Read {
                    constructor: s.constructor.as_ref().map(ToString::to_string),
                    uri: s.uri.as_ref().map(ToString::to_string),
                },
                |p| SourceRhs::Derive {
                    pipe: pipe_text(p),
                    inline: false,
                },
            ),
        })
        .collect();

    // Then the pipelines that are in the HIR and NOT among the bindings: the
    // ones written in a mapping header, which `lower_to_hir` registers under the
    // mapping's name. In written order after the bindings, because that is where
    // a mapping is — no program can write one before its sources exist.
    sources.extend(
        pipes
            .iter()
            .filter(|p| !dm.sources(&db).iter().any(|s| s.name == p.name))
            .map(|p| SourceBinding {
                name: p.name.to_string(),
                rhs: SourceRhs::Derive {
                    pipe: pipe_text(p),
                    inline: true,
                },
            }),
    );

    let mappings = dm
        .mappings(&db)
        .iter()
        .map(|loc| {
            let node = mapping_cst_node(&db, *loc).syntax();
            let name = node.as_ref().map_or_else(
                || "«no CST node»".to_string(),
                |n| {
                    n.children()
                        .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)
                        .and_then(|h| {
                            h.children_with_tokens()
                                .filter_map(fossil_syntax::SyntaxElement::into_token)
                                .find(|t| t.kind() == SyntaxKind::IDENT)
                                .map(|t| t.text().to_string())
                        })
                        .unwrap_or_else(|| "«unnamed»".to_string())
                },
            );
            let (written, spans): (Vec<String>, Vec<(u32, u32)>) = node
                .as_ref()
                .and_then(|n| n.children().find(|c| c.kind() == SyntaxKind::MAPPING_BODY))
                .map(|b| {
                    b.children()
                        .filter(|c| c.kind() == SyntaxKind::PROPERTY)
                        .map(|p| {
                            let r = p.text_range();
                            (
                                written_key(&p.text().to_string()),
                                (r.start().into(), r.end().into()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let lowered = body(&db, *loc)
                .properties(&db)
                .iter()
                .map(|p| match &p.key {
                    PropertyKey::Subject => "@subject".to_string(),
                    PropertyKey::Name(n) => n.to_string(),
                })
                .collect();
            MappingCensus {
                name,
                written,
                spans,
                lowered,
            }
        })
        .collect();

    Ok(ProgramCensus {
        types,
        sources,
        mappings,
    })
}

/// The key of a property as written: everything before the first `=` that is
/// not part of `==`, `!=`, `<=` or `>=`.
///
/// `Property := PropertyLhs ASSIGN Expression`, so the first `=` in the node
/// text ends the key — unless the parser recovered badly enough that the node
/// swallowed a following line, which is why the result is trimmed of comments
/// and newlines rather than assumed to be one line.
fn written_key(text: &str) -> String {
    let head = text
        .find('=')
        .map_or(text, |i| &text[..i])
        .lines()
        .next_back()
        .unwrap_or("")
        .trim();
    if head.is_empty() {
        format!("«no key in `{}`»", text.trim().replace('\n', "⏎"))
    } else {
        head.to_string()
    }
}
