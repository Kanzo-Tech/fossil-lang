//! What the compiler UNDERSTOOD from a program — the reading, not the verdict.
//!
//! [`crate::check`] answers «is anything wrong», and a program can be entirely
//! right about nothing. The measured hole this module exists to close: a
//! property whose right-hand side the lowering cannot read is dropped in
//! silence. [`fossil_hir::body::body`] walks the `PROPERTY` children of a
//! mapping body and keeps the ones `lower_property` returns `Some` for; the
//! `None` arm is a bare `if let`, with no `else` and no accumulator, so a
//! mapping that wrote five properties and lowered two produces a graph missing
//! three columns and a `check` that says `ok`.
//!
//! Nothing downstream can see that. `check` drains diagnostics and there are
//! none; `run` writes what it was given and the missing columns were never in
//! the MIR to be missed. The only place the difference is visible is between
//! the CST and the `HirBody`, which is where this looks.
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
use fossil_hir::lower::PropertyKey;
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

/// One `Name := io.csv("…")` binding, and what the signature reading made of it.
#[derive(Debug, Clone)]
pub struct SourceBinding {
    pub name: String,
    /// `io.csv` / `io.json` / … — `None` when the RHS is not a recognisable call.
    pub constructor: Option<String>,
    /// The first positional string argument.
    pub uri: Option<String>,
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
            let _ = writeln!(
                out,
                "{} := {}({})",
                s.name,
                s.constructor.as_deref().unwrap_or("?"),
                s.uri.as_deref().unwrap_or("?"),
            );
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

    let sources = dm
        .sources(&db)
        .iter()
        .map(|s| SourceBinding {
            name: s.name.to_string(),
            constructor: s.constructor.as_ref().map(ToString::to_string),
            uri: s.uri.as_ref().map(ToString::to_string),
        })
        .collect();

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
