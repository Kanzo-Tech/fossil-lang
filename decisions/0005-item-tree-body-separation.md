# ADR 0005: ItemTree carries signatures only; bodies live behind per-mapping body() query

**Date:** 2026-05-18
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/02-full-grammar-hir-foundation/02-RESEARCH.md` §Q3 (ItemTree design) + §Q8 (Salsa invalidation regression test); rust-analyzer `crates/hir-def/src/item_tree.rs` lines ~216-224; `.planning/ROADMAP.md` CORE-02 SC#2

## Context

Phase 1 shipped a placeholder `ItemTree { item_count: usize }` (commit `65b7055`) whose doc comment explicitly anticipated Phase 2's "stable across body edits" expansion. ROADMAP CORE-02 success criterion #2 reads verbatim:

> Editing one character in the body of mapping #3 of a 10-mapping file does NOT invalidate `item_tree(file)` — Salsa invalidation regression test, fails CI if re-execution count exceeds threshold.

The architectural question is: what data structure does `ItemTree` carry such that this property is achievable? Three options were on the table per Phase 2 RESEARCH.md §Q3:

- **Option (a) — Bake bodies into ItemTree.** Each `ItemHeader` carries a full `Vec<HirProperty>`. Simple for consumers (one query gets everything). Trivially fails SC#2 — Salsa's structural-equality check on `ItemTree`'s tracked output fires on every body edit because the property values change.

- **Option (b) — Split: `ItemTree` = signatures only; `body()` = per-mapping Salsa query keyed by `MappingLoc`.** Bodies live behind `body(db, MappingLoc<'db>) -> HirBody<'db>`, a separate `#[salsa::tracked]` query. `ItemTree`'s input dependency is the top-level structure only (kinds + signature tokens + structural counts), so body-only edits do not invalidate it. Pattern: rust-analyzer `crates/hir-def/src/item_tree.rs` plus the per-item `body_with_source_map` query in `crates/hir-def/src/body.rs`.

- **Option (c) — Hybrid: `ItemTree` carries body hashes (not contents).** Saves a query lookup at the cost of re-hashing on every body parse. Doesn't actually help invalidation: if body content changes, the hash changes, `ItemTree` invalidates — same outcome as option (a).

Phase 1 SUMMARY explicitly flagged this decision as the single highest-priority Phase 2 design call: *"Likely ADR-0005 BEFORE plan derivation."* RESEARCH.md §Q3 reaches the same conclusion citing the rust-analyzer reference.

The forces in tension: data-model simplicity (one query returning everything) versus the CORE-02 invalidation-barrier property. The reference architecture (rust-analyzer) chose the split; ty/red-knot followed the same pattern; ruff_db too. No competing reference implementation uses option (a) for a Salsa-based IDE backend.

## Decision

We will adopt option (b). Implement `ItemTree<'db>` as `Vec<ItemHeader>` where every `ItemHeader` variant carries SIGNATURE data only:

- identifier names (`name: SmolStr`)
- surface text of shape IRIs (`shape_iris: Vec<SmolStr>` — prefix-resolution is downstream)
- source binding name (`source_binding: Option<SmolStr>`)
- structural counts (`body_property_count: u32` — counting IS allowed because adding/removing a property IS a structural change)

Bodies live behind `pub fn body<'db>(db: &'db dyn Db, mapping: MappingLoc<'db>) -> HirBody<'db>`. `HirBody` carries the `Vec<HirProperty>` that Phase 1's `HirMapping.properties` previously held, plus (Phase 2 plan 02-06) a per-mapping `ExprId` arena bookkeeping field for the type-provenance side table.

Additionally: `AstIdMap<'db>` provides stable typed indices (`FileAstId<N>`) into the CST so an `ItemHeader`'s `ast_id` field remains a valid pointer to its node across body edits in sibling mappings. Indices are assigned in DFS top-level scan order; rowan's green-node reuse keeps the indices stable when only body content changes.

The `body()` query MUST filter CST children to `SyntaxKind::MAPPING` BEFORE applying `.nth(idx)`. The opposite ordering (`.nth(idx).filter(...)`) silently returns the wrong mapping when a file has non-MAPPING top-level siblings (`prefix`, `source_def`, `import`) before the target. The regression test `body_filters_to_mapping_kind_before_indexing` in `crates/fossil-hir/src/body.rs` enforces this.

`def_map.rs`'s `MappingLoc.index` allocation MUST use the same filter convention — position among MAPPING-kind children only, not the all-children index. This was already correct after plan 02-03's per-kind dense indexing change; plan 02-04 documents the contract explicitly in a comment block above the allocation loop.

## Consequences

**Positive:**

- CORE-02 SC#2 is structurally achievable. Wave 4 plan 02-07's Salsa-event-count regression test (10-mapping fixture from plan 02-01, edit one character in mapping #3's body, count `EventKind::WillExecute` events, assert ≤4) will pass because `ItemTree`'s input does not include body content.
- Per-mapping fan-out: editing mapping #N's body invalidates exactly `body(M_N) + typecheck_mapping(M_N) + lower_to_mir(M_N) + codegen_sql(M_N)`. Sibling mappings stay cached, supporting Phase 6's sub-100ms LSP perf goal (LSP-02).
- Direct alignment with rust-analyzer's reference architecture. Proven at scale (millions of LOC of Rust code in IDE sessions), well-documented (the rust-analyzer book has a dedicated "Architecture: Salsa and Item Tree" chapter), patterns are easy to copy when adding new item kinds.
- `AstIdMap` makes future cross-file references (Phase 3 `import` resolution) cheap: imports name a target by `(file, FileAstId<DefinitionNode>)`, which survives unrelated edits in the target file.

**Negative:**

- Two queries instead of one. Consumers that need both signature + body (e.g. `fossil-mir::lower_to_mir`) must call `lower_to_hir` for the header AND `body(db, MappingLoc)` for the property list — an extra Salsa query per mapping. Phase 1 was a single `lower_to_hir(file).mappings(db)` traversal.
- API change visible to `fossil-mir` and (future) `fossil-codegen` consumers: the lowering pipeline must fetch `body()` per `MappingLoc` instead of reading `HirMapping.properties` directly. Walking-skeleton invariant is enforced via `cargo test -p fossil-cli --test walking_skeleton`; plan 02-04 verified zero regressions end-to-end.
- `HirMapping` shrinks — removing `properties: Vec<HirProperty>` is a breaking change to Phase 1's `HirFile` shape. Done all-at-once in plan 02-04 per the CLAUDE.md walking-skeleton hard rule (no >3-day regression window).
- The MAPPING-kind filter contract in `body()` and `def_map.rs` is a non-obvious shared invariant. If either side is changed independently to use the all-children index, `body()` will silently return the wrong CST subtree (downstream queries operate on the wrong content with no panic). Mitigation: the regression test `body_filters_to_mapping_kind_before_indexing` covers it, and `def_map.rs` carries a comment block citing the contract.

**Neutral:**

- The `body_property_count: u32` field in `MappingHeader` is a deliberate structural-vs-content boundary. Adding/removing a property invalidates `ItemTree` (correct — that's a structural edit visible to outline / document-symbol). Editing an existing property's value does not (correct — same count, same names in the header).
- `lower_to_hir`'s mappings list is now header-only. A consumer that needs both the header and the body for every mapping must do a per-mapping `body()` call. For files with many mappings this is N queries, but each is `O(1)` after the first (Salsa memoised), so the practical cost is one query per mapping per session, not per access.

## Alternatives considered

- **Option (a) — baked bodies:** rejected. SC#2 unsatisfiable; trivially regresses every body edit through every downstream query.
- **Option (c) — body hashes in `ItemTree`:** rejected. Doesn't change invalidation semantics (hash changes → tracked output changes → consumers invalidate). Adds re-hashing cost on every body parse.
- **Per-property invalidation (smaller granularity):** rejected for Phase 2. Would require per-property interned location IDs (`PropertyLoc`), a second per-item interning layer with its own dense-index allocation rules. Phase 2 keeps the granularity at MAPPING level (matches rust-analyzer). Phase 6 may revisit if LSP perf budgets demand it (LSP-02).
- **Not using `AstIdMap` (just `SyntaxNodePtr` per `ItemHeader`):** rejected. `SyntaxNodePtr` is range-keyed, so any byte-offset shift invalidates the pointer (every edit in the file changes downstream offsets). `AstIdMap`'s DFS-order assignment makes indices stable under body edits — rust-analyzer PR #4001 documents this trade-off explicitly.

## Implementation

Landed in Phase 2 plan 02-04, commit `796b122` (`feat(fossil-hir,fossil-mir)(02-04): split ItemTree signatures from body() query per ADR-0005`).

Files:

- `crates/fossil-hir/src/ast_id.rs` — new
- `crates/fossil-hir/src/item_tree.rs` — rewritten (placeholder → `Vec<ItemHeader>`)
- `crates/fossil-hir/src/body.rs` — new
- `crates/fossil-hir/src/lower.rs` — `HirMapping.properties` REMOVED; `lower_property_public` exposed for `body.rs` reuse
- `crates/fossil-hir/src/def_map.rs` — comment block documenting the MAPPING-kind index contract
- `crates/fossil-hir/src/lib.rs` — module + type re-exports
- `crates/fossil-mir/src/lower.rs` — consumer migrated from `HirMapping.properties` to `body(db, mapping)`

## Validation

CORE-02 SC#2 will be mechanically proven by `crates/fossil-hir/tests/invalidation_regression.rs::editing_body_of_mapping_3_does_not_invalidate_item_tree`, landing in Phase 2 plan 02-07. The 10-mapping baseline fixture + one-char body-edit fixture pair lives at `crates/fossil-hir/tests/fixtures/ten_mappings_{baseline,mapping_3_body_one_char_edit}.fossil` (created in plan 02-01). Threshold: ≤4 `EventKind::WillExecute` events for the body-only edit path (`parse + body(M_3) + typecheck_mapping(M_3) + expr_types(M_3)`).

Plan 02-04 lands the structural prerequisite. Plan 02-04 also includes the lower-stakes `body_filters_to_mapping_kind_before_indexing` regression test that proves the filter-before-nth correctness contract on a 3-mapping fixture with distinct property counts.

Walking-skeleton invariant intact: `cargo test -p fossil-cli --test walking_skeleton` PASS after the migration. WASM gate green for all 6 gated crates.

## References

- rust-analyzer source: `crates/hir-def/src/item_tree.rs` lines ~216-224 (signature-only layout); `crates/hir-def/src/body/lower.rs` (per-item body query)
- rust-analyzer book — "Architecture" chapter, "Salsa and Item Tree" section
- Phase 2 RESEARCH.md §Q3 (ItemTree design — recommends option (b) verbatim)
- Phase 2 RESEARCH.md §Q8 (Salsa invalidation regression test design)
- Phase 1 SUMMARY ("Phase 2 starting position" note: "Single most important Phase 2 design call: item-tree boundary semantics (CORE-02 SC#2)")
- ADR-0003 (thin Db trait + System abstraction) — context for the `'db` lifetime carried by `ItemTree<'db>`, `HirBody<'db>`, `AstIdMap<'db>`
- ADR-0004 (unsafe_code = deny, not forbid) — permits the per-item `#[allow(unsafe_code)] unsafe impl salsa::Update for FileAstId<N>` in `ast_id.rs` (third-party-trait integration boundary, with one-line justification per ADR-0004 policy)
