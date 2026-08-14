//! Per-mapping real-span side table — Phase 3 plan 03-04.
//!
//! Replaces Phase 2's zero-width `Span { start: 0, end: 0 }` placeholders for
//! literal-subset provenance entries (see plan 02-06) with real byte ranges
//! read from `rowan::TextRange` during lowering.
//!
//! # Why a side table (not a field on `HirExpr`)
//!
//! Same architectural rationale as the Phase 2 provenance side table
//! (`crate::provenance`): adding `span` to every `HirExpr` variant would
//! change its `PartialEq`/`Hash`, which Salsa uses for `salsa::Update`. Two
//! source-identical expressions at different source positions would no
//! longer dedupe in interning, defeating the "structural equality at the
//! HIR layer → pointer equality after Salsa interning" contract.
//!
//! Three options were considered:
//! 1. Add `span: Span` field to every `HirExpr` variant. REJECTED — breaks
//!    Salsa interning.
//! 2. Per-mapping `Spans<'db>` side table keyed by `(MappingLoc, ExprId)`,
//!    populated during lowering. CHOSEN.
//! 3. On-demand span lookup at diagnostic emission time via CST walk.
//!    REJECTED — wasteful (re-walks the CST every time a diagnostic is
//!    emitted) and the lowering arena is the right place to record
//!    positions since it's already navigating the CST node-by-node.
//!
//! # Salsa invalidation barrier
//!
//! The [`spans`] tracked query reads [`crate::body::body`], which reads
//! [`mapping_cst_node`]. This is REQUIRED so the Phase 2 plan 02-07
//! `MAX_PER_MAPPING_FAN_OUT = 1` invariant continues to hold. Reading
//! `parse(db, file)` directly would tie every per-mapping spans query to the
//! whole-file CST, causing all sibling spans to re-execute on any body edit.
//! The intermediate per-mapping CST barrier (plan 02-07) prevents
//! this — see `crates/fossil-hir/src/body.rs`'s `mapping_cst_node`
//! documentation.
//!
//! It used to walk the barrier itself, in parallel with `body`, and assign its
//! own `ExprId`s from a different rule. See [`spans`].
//!
//! # `ExprId` convention
//!
//! `ExprId(i)` is the i'th LOWERED property's RHS expression — the index into
//! `crate::body::HirBody::properties`, and nothing else. It is not the position
//! among the CST's `PROPERTY` children, and this module used to assume it was.
//! Subexpression-level arena allocation (for nested function calls, ternaries,
//! etc.) lands when the Pratt-lowered expression tree extends `HirExpr`.
//!
//! # Offset semantics: mapping-relative
//!
//! The recorded [`Span`]s are RELATIVE to the start of the mapping (not
//! file-absolute). This is a direct consequence of reading
//! [`mapping_cst_node`] rather than `parse(file)`: rowan's
//! `SyntaxNode::new_root(green)` resets offsets to zero, so
//! `text_range()` values inside a per-mapping subtree are mapping-relative.
//!
//! This is the right trade-off — file-absolute offsets would require
//! reading `parse(db, file)` directly, defeating the per-mapping
//! invalidation barrier. Mapping-relative offsets are stable across
//! sibling-mapping edits (only the mapping's own content moves spans;
//! shifts in OTHER mappings don't cause `Spans(M_k)` to re-execute).
//!
//! The diagnostic-emission layer (`crate::check::compatible` in plan
//! 03-05, the LSP host) is responsible for converting mapping-relative
//! offsets to file-absolute when needed. The conversion is a single
//! lookup of the mapping's start offset in the file CST and an add.
//!
//! # Lookup complexity
//!
//! [`Spans::get`] is a linear scan over `Vec<(ExprId, Span)>`. Typical
//! mapping has ≤50 expressions; for the LSP hover hot path the lookup is
//! one `expr_id` per request. If profiling later shows this hot, swap for
//! `HashMap<u32, Span>` or sorted-Vec + binary search — the public API
//! shape (`spans(db, m).get(db, expr_id)`) stays unchanged.

use fossil_base::Span;

use crate::body::{ExprId, mapping_cst_node};
use crate::def_map::MappingLoc;

/// Per-mapping real-span side table.
///
/// Indexed-by-position into `ExprId(u32)` → vector position. The
/// `crate::check::compatible` two-span blame in plan 03-05 reads this via
/// [`Spans::get`] for both source and destination blame positions.
#[salsa::tracked(debug)]
pub struct Spans<'db> {
    /// Per-expression spans. One entry per Phase 2 property RHS (matches
    /// the `expr_id = property_index` convention established in plan 02-06).
    #[returns(ref)]
    pub by_expr: Vec<(ExprId, Span)>,
}

impl<'db> Spans<'db> {
    /// Look up the real byte range for an [`ExprId`]. Returns `None` for
    /// unknown ids (defensive — Phase 3 callers should always have a valid
    /// id from `body(db, mapping).expr_count(db)`).
    #[must_use]
    pub fn get(self, db: &'db dyn fossil_base::Db, expr_id: ExprId) -> Option<Span> {
        self.by_expr(db)
            .iter()
            .find(|(id, _)| *id == expr_id)
            .map(|(_, s)| *s)
    }
}

/// Per-mapping spans query — an ACCESSOR over [`crate::body::HirBody`]'s
/// `expr_spans`, which is where the ranges are recorded.
///
/// This used to walk the CST a second time and number the properties by their
/// POSITION among the `PROPERTY` children, via an `.enumerate()`. Its own
/// comment said the opposite — «Skip properties that wouldn't have lowered to
/// an `ExprId` … Spans matches `body()`'s indexing» — and the code did not skip
/// anything: the counter advanced on every CST child, lowered or not. `body()`
/// numbers densely over the ones that DID lower, so the two agreed exactly
/// until one property failed to lower, and then every id after it was off by
/// one. The checker mints ids with `body()`'s numbering and looks them up here,
/// so a type error underlined the wrong property — silently, and most often on
/// the corpus's retired spellings, which are precisely the properties that
/// parse and do not lower.
///
/// Reading `body` rather than `mapping_cst_node` also removes a walk: the
/// per-mapping invalidation barrier is unchanged (plan 02-07),
/// because `body` reads the same barrier and this query now reads only `body`.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn spans<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> Spans<'db> {
    let by_expr: Vec<(ExprId, Span)> = crate::body::body(db, mapping)
        .expr_spans(db)
        .iter()
        .enumerate()
        .map(|(i, span)| (ExprId(u32::try_from(i).unwrap_or(u32::MAX)), *span))
        .collect();
    Spans::new(db, by_expr)
}

/// The mapping-relative [`Span`] of a mapping's HEADER — `User : ex:Person
/// from users`.
///
/// The span every diagnostic about the mapping AS A WHOLE belongs on: the
/// target shape it names and could not resolve, two of its predicates sharing a
/// short name, a predicate its shape requires and its body never wrote. None of
/// those blames one expression, and all three used to carry
/// `Span { start: 0, end: 0 }`.
///
/// **A zero-width span here is not "no underline".** The default
/// [`SpanFrame`](fossil_base::SpanFrame) is `MappingRelative`, so
/// [`rebase_to_file`] turns `0..0` into `base..base` — the first byte of the
/// mapping, which is a plausible place and the wrong one.
///
/// Plain-Rust, and it costs no fan-out: it reads
/// [`mapping_cst_node`] — the same per-mapping barrier [`spans`] already reads
/// (plan 02-07), memoized per mapping — never `parse(db, file)`.
/// The walk is [`crate::body`]'s, which reads the header for the mapping's own
/// name.
///
/// Falls back to `0..0` when the mapping index resolves to no CST node or the
/// node carries no header — a parse failure, where there is nothing to point
/// at.
#[must_use]
pub fn mapping_header_span<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> Span {
    use fossil_syntax::SyntaxKind;

    mapping_cst_node(db, mapping)
        .syntax()
        .and_then(|node| {
            node.children()
                .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)
        })
        .map_or(Span { start: 0, end: 0 }, |header| {
            let range = header.text_range();
            // The header node swallows the newline and the next line's indent
            // (they are its trailing trivia), and an underline that runs into
            // the body reads as a claim about the body. Trim back to the last
            // non-whitespace byte.
            let text = header.text().to_string();
            let trailing = u32::try_from(text.len() - text.trim_end().len()).unwrap_or(0);
            Span {
                start: u32::from(range.start()),
                end: u32::from(range.end()).saturating_sub(trailing),
            }
        })
}

/// Byte offset of a mapping's CST node within its file.
///
/// The counterpart to this module's mapping-relative offsets: add this to any
/// recorded [`Span`] to get a file-absolute one.
///
/// DELIBERATELY NOT a `#[salsa::tracked]` query. It reads `parse(db, file)`,
/// which is exactly the whole-file read the per-mapping barrier exists to keep
/// out of the compile path. Callers are diagnostic-EMISSION layers,
/// which sit outside that barrier and already hold the file text.
#[must_use]
pub fn mapping_start_offset<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> u32 {
    fossil_syntax::parse(db, mapping.file(db))
        .root(db)
        .syntax()
        .children()
        .filter(|n| n.kind() == fossil_syntax::SyntaxKind::MAPPING)
        .nth(mapping.index(db))
        .map_or(0, |n| u32::from(n.text_range().start()))
}

/// Rebase a mapping's diagnostics from mapping-relative onto file-absolute
/// offsets, so a host can render them against the file text.
///
/// EVERY diagnostic-emission layer must call this. Skipping it does not fail
/// loudly — it silently points the squiggle at whatever happens to sit at that
/// offset from the start of the FILE, which for any mapping but the first is
/// another mapping entirely. It rebases [`Diagnostic::did_you_mean`]'s
/// `wrong_span` too: that one drives a quick-fix `WorkspaceEdit`, so a stale
/// offset there does not just mislead, it edits the wrong range.
#[must_use]
pub fn rebase_to_file<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    diagnostics: impl IntoIterator<Item = fossil_base::Diagnostic>,
) -> Vec<fossil_base::Diagnostic> {
    let base = mapping_start_offset(db, mapping);
    let shift = |s: Span| Span {
        start: s.start.saturating_add(base),
        end: s.end.saturating_add(base),
    };
    diagnostics
        .into_iter()
        .map(|mut d| {
            // Each part is shifted by ITS OWN frame, not by the diagnostic's.
            // A `SpanLabel` on a two-mapping report points at the OTHER mapping
            // and is therefore already file-absolute, inside a diagnostic that
            // is not — see `fossil_base::SpanLabel`. The two used to be one
            // decision because there was only one span.
            for label in &mut d.labels {
                if label.frame == fossil_base::SpanFrame::MappingRelative {
                    label.span = shift(label.span);
                }
            }
            // A file-level diagnostic that happens to be emitted from a
            // per-mapping query is already absolute — shifting it by the
            // mapping's start would move it somewhere meaningless.
            if d.frame == fossil_base::SpanFrame::FileAbsolute {
                return d;
            }
            d.span = shift(d.span);
            if let Some(dym) = d.did_you_mean.as_mut() {
                dym.wrong_span = shift(dym.wrong_span);
            }
            d
        })
        .collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::def_map::def_map;
    use std::sync::Arc;

    fn db_with_text(src: &str, name: &str) -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), name.to_string());
        (db, file)
    }

    /// `ExprId(i)` selects `properties[i]`'s right-hand side. One numbering.
    ///
    /// # What this proves
    ///
    /// That the span table and the property vector are indexed by the same
    /// thing, in both mappings of a two-mapping file: for each `ExprId` it
    /// slices the mapping's own text with the recorded span and compares the
    /// slice to the right-hand side written in the fixture.
    ///
    /// There were THREE notions of `ExprId` and they were equal only while
    /// every property lowered. This query numbered by CST position, from an
    /// `.enumerate()`, under a comment claiming it matched `body()`;
    /// `body()` numbers densely over the properties that DID lower; and
    /// `fossil_ide::hover` used the CST position for BOTH the id and an index
    /// into `properties`. One property that parses and does not lower — every
    /// retired spelling in the corpus is one — makes the CST longer than the
    /// HIR and shifts everything behind it. A type error then underlined a
    /// different property and a hover named a different predicate, and neither
    /// failed loudly: both point somewhere plausible.
    ///
    /// # What it CANNOT prove
    ///
    /// - **That the fixture contains a property which fails to lower.** That is
    ///   the case the whole thing turns on, and it cannot be written down
    ///   stably while the grammar is being cut: the spellings that parse and
    ///   refuse to lower are exactly the ones being removed. What is asserted
    ///   here is the invariant that makes the skew impossible — one span per
    ///   LOWERED property, recorded where the property is lowered — not a
    ///   reproduction of the old failure.
    /// - **That the spans are the right ranges** for any purpose other than
    ///   agreeing with each other. A `body()` that consistently recorded the
    ///   key instead of the value would pass the length check; the slice
    ///   comparison below is what stops that.
    /// - **Anything below a property.** `ExprId` is per property, so a hover
    ///   inside a call argument still resolves to the whole property.
    #[test]
    fn expr_id_selects_the_same_property_in_both_tables() {
        const SRC: &str = "\
type { Person, Order } = io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")
Row := io.csv(\"orders.csv\")

Users : Person from User
    @subject = \"https://e.org/u/{User.id}\"
    name = User.name

Orders : Order from Row
    @subject = \"https://e.org/o/{Row.id}\"
    total = Row.amount
    note = Row.note
";
        // The right-hand sides, per mapping, in source order — what `ExprId(i)`
        // must select.
        let expected: [&[&str]; 2] = [
            &["\"https://e.org/u/{User.id}\"", "User.name"],
            &["\"https://e.org/o/{Row.id}\"", "Row.amount", "Row.note"],
        ];

        let (db, file) = db_with_text(SRC, "align.fossil");
        let mappings = def_map(&db, file).mappings(&db).clone();
        assert_eq!(mappings.len(), 2, "two mappings");

        for (m, want) in mappings.into_iter().zip(expected) {
            let text = mapping_text(&db, m);
            let hir = crate::body::body(&db, m);
            let props = hir.properties(&db);
            let table = spans(&db, m);

            assert_eq!(
                hir.expr_spans(&db).len(),
                props.len(),
                "one span per LOWERED property, or the index is not the ExprId"
            );
            assert_eq!(
                props.len(),
                want.len(),
                "the fixture must lower every property it writes, or this test \
                 is asserting alignment over a shorter list than it thinks: \
                 got {props:#?}"
            );

            for (i, wanted) in want.iter().enumerate() {
                let id = ExprId(u32::try_from(i).expect("small"));
                let span = table
                    .get(&db, id)
                    .unwrap_or_else(|| panic!("no span recorded for {id:?}"));
                let slice = text
                    .get(span.start as usize..span.end as usize)
                    .unwrap_or_else(|| panic!("{span:?} is not inside the mapping text"));
                assert_eq!(
                    slice, *wanted,
                    "ExprId({i}) must select the right-hand side of the property \
                     at index {i} — the two tables are indexed by the same number \
                     or they are indexed by nothing"
                );
            }
        }
    }

    /// The header span is the header, and it is not `0..0`.
    ///
    /// `surface_target_shape_error`, `surface_name_collisions` and
    /// `check_required_properties` all emitted `Span { start: 0, end: 0 }`, and
    /// that is not «no underline»: the default frame is `MappingRelative`, so
    /// [`rebase_to_file`] turns it into `base..base` and the squiggle lands on
    /// the mapping's first byte — which is a plausible place and the wrong one.
    ///
    /// It cannot prove the range is what an editor should highlight, only that
    /// it covers the header text and stops before the body.
    #[test]
    fn the_header_span_covers_the_header_and_nothing_else() {
        const SRC: &str = "\
type { Person } = io.shex(\"shop.shex\")
User := io.csv(\"users.csv\")

Users : Person from User
    @subject = \"https://e.org/u/{User.id}\"
    name = User.name
";
        let (db, file) = db_with_text(SRC, "hdr.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let span = mapping_header_span(&db, m);
        let text = mapping_text(&db, m);
        assert_eq!(
            text.get(span.start as usize..span.end as usize),
            Some("Users : Person from User"),
            "the header span must select the header, with no trailing trivia"
        );
    }

    /// A rebased span must select the SAME text from the file that the raw
    /// span selects from the mapping. Regression guard: before
    /// [`rebase_to_file`], every emission layer rendered mapping-relative
    /// offsets against the file, so a diagnostic in any mapping but the first
    /// pointed at an unrelated earlier one.
    #[test]
    fn rebase_lands_on_the_same_text_in_the_file() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
First : ex:A from users
    @subject = `${ex:}a/${.id}`
    name = User.name

Second : ex:B from users
    @subject = `${ex:}b/${.id}`
    name = .other
";
        let (db, file) = db_with_text(SRC, "two.fossil");
        let second = *def_map(&db, file)
            .mappings(&db)
            .get(1)
            .expect("Second is the 2nd mapping");

        let raw = spans(&db, second)
            .get(&db, ExprId(1))
            .expect("the 2nd property's RHS");
        let local = &mapping_text(&db, second)[raw.start as usize..raw.end as usize];
        assert_eq!(local, ".other", "sanity: the raw span is mapping-relative");

        let base = mapping_start_offset(&db, second);
        assert!(base > 0, "the 2nd mapping does not start at the file head");
        let absolute = &SRC[(raw.start + base) as usize..(raw.end + base) as usize];
        assert_eq!(
            absolute, local,
            "rebased span selects the same text in the file"
        );

        // And the whole-diagnostic path shifts `did_you_mean.wrong_span` too —
        // that one drives a quick-fix edit, so a stale offset corrupts source.
        let d = fossil_base::Diagnostic::new(fossil_base::Severity::Error, "x", raw)
            .with_did_you_mean(raw, "name");
        let out = rebase_to_file(&db, second, [d]);
        assert_eq!(out[0].span.start, raw.start + base);
        assert_eq!(
            out[0]
                .did_you_mean
                .as_ref()
                .expect("did_you_mean survives")
                .wrong_span
                .start,
            raw.start + base
        );
    }

    /// A `FileAbsolute` diagnostic must survive rebasing untouched. Some
    /// diagnostics emitted from per-mapping queries are about FILE-level syntax
    /// (a `SOURCE_DEF`'s `schema = "..."`, which lives outside every mapping);
    /// shifting those by the mapping's start moves them onto unrelated text.
    #[test]
    fn rebase_leaves_file_absolute_diagnostics_alone() {
        const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
First : ex:A from users
    @subject = `${ex:}a/${.id}`
    name = User.name

Second : ex:B from users
    @subject = `${ex:}b/${.id}`
    name = .other
";
        let (db, file) = db_with_text(SRC, "two.fossil");
        let second = *def_map(&db, file)
            .mappings(&db)
            .get(1)
            .expect("2nd mapping");
        assert!(
            mapping_start_offset(&db, second) > 0,
            "a shift is available"
        );

        let span = Span::new(3, 9);
        let out = rebase_to_file(
            &db,
            second,
            [
                fossil_base::Diagnostic::new(fossil_base::Severity::Error, "file-level", span)
                    .file_absolute(),
                fossil_base::Diagnostic::new(fossil_base::Severity::Error, "mapping-level", span),
            ],
        );
        assert_eq!(out[0].span, span, "file-absolute spans are never shifted");
        assert_ne!(out[1].span, span, "mapping-relative spans are");
    }

    /// Helper — return the mapping's own text (the substring of `src`
    /// covered by `mapping_cst_node`). Spans are mapping-relative, so the
    /// tests slice into this string, NOT into the full file source. Per
    /// the offset-semantics doc-comment at the top of this module.
    fn mapping_text<'db>(db: &'db fossil_base::FossilDb, m: MappingLoc<'db>) -> String {
        let cst = mapping_cst_node(db, m);
        cst.syntax()
            .expect("mapping CST node must exist")
            .text()
            .to_string()
    }

    /// Property 0 of `hello.fossil` is the iri template property. Phase 2
    /// synthesises a `Template` RHS for it. The recorded mapping-relative
    /// span MUST cover the entire RHS expression (backtick to backtick
    /// inclusive) and be non-zero width.
    #[test]
    fn spans_for_template_property() {
        const SRC: &str = "\
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
    name = User.name
";
        let (db, file) = db_with_text(SRC, "tpl.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        let span = s
            .get(&db, ExprId(0))
            .expect("property 0 (iri = template) must have a recorded span");
        assert!(
            span.end > span.start,
            "Template span must be non-zero width: {span:?}"
        );
        // Spans are mapping-relative; slice into the mapping's own text.
        let text = mapping_text(&db, m);
        let extracted = &text[span.start as usize..span.end as usize];
        assert!(
            extracted.starts_with('`'),
            "Template span must start at the opening backtick, got {extracted:?}"
        );
        assert!(
            extracted.ends_with('`'),
            "Template span must end at the closing backtick, got {extracted:?}"
        );
        assert!(
            extracted.contains("${.id}"),
            "Template span must cover the full backtick-delimited body, got {extracted:?}"
        );
    }

    /// Property 1 of `hello.fossil` is `name = .name` — a `FieldRef`
    /// RHS. The recorded mapping-relative span MUST cover `.name`
    /// (5 chars including the leading dot) and be non-zero width.
    #[test]
    fn spans_for_field_ref() {
        const SRC: &str = "\
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
    name = User.name
";
        let (db, file) = db_with_text(SRC, "fref.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        let span = s
            .get(&db, ExprId(1))
            .expect("property 1 (name = .name) must have a recorded span");
        assert!(
            span.end > span.start,
            "FieldRef span must be non-zero width: {span:?}"
        );
        let text = mapping_text(&db, m);
        let extracted = &text[span.start as usize..span.end as usize];
        assert_eq!(
            extracted, ".name",
            "FieldRef span must cover exactly `.name`, got {extracted:?}"
        );
    }

    // `spans_for_prefixed_name_rhs` lived here: `link = ex:Foo` had to record a
    // span covering the six characters exactly. What it proved about spans, the
    // `StringLit` test below proves on a form the language still has.

    /// `greeting = "Alice"` exercises the `StringLit` RHS form. The
    /// recorded mapping-relative span MUST cover the literal INCLUDING
    /// the surrounding quotes — the `EXPR` CST node's `text_range()` is
    /// the lex span, which is quote-inclusive.
    #[test]
    fn spans_for_string_lit_property() {
        const SRC: &str = "\
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
    greeting = \"Alice\"
";
        let (db, file) = db_with_text(SRC, "lit.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        let span = s
            .get(&db, ExprId(1))
            .expect("property 1 (greeting = \"Alice\") must have a recorded span");
        assert!(
            span.end > span.start,
            "StringLit span must be non-zero width: {span:?}"
        );
        let text = mapping_text(&db, m);
        let extracted = &text[span.start as usize..span.end as usize];
        assert_eq!(
            extracted, "\"Alice\"",
            "StringLit span must cover the quoted literal including quotes, \
             got {extracted:?}"
        );
    }

    /// Sanity: `Spans::get` returns `None` for an unknown `ExprId` rather
    /// than panicking. Phase 3 callers (the bidirectional checker, hover
    /// handler) lean on this for graceful degradation.
    #[test]
    fn spans_get_returns_none_for_unknown_expr_id() {
        const SRC: &str = "\
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
";
        let (db, file) = db_with_text(SRC, "unk.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let s = spans(&db, m);
        // Only one property → ExprId(0). ExprId(99) must return None.
        assert!(s.get(&db, ExprId(0)).is_some());
        assert!(s.get(&db, ExprId(99)).is_none());
    }

    /// Memoisation sanity check — `Spans` is salsa-tracked, so repeated
    /// invocations on the same `MappingLoc` yield the same handle.
    #[test]
    fn spans_is_memoised_per_mapping() {
        const SRC: &str = "\
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
    name = User.name
";
        let (db, file) = db_with_text(SRC, "memo.fossil");
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let a = spans(&db, m);
        let b = spans(&db, m);
        assert_eq!(a, b);
    }
}
