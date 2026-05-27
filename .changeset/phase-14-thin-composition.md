---
"@fossil-lang/playground": minor
---

Playground thin composition (Phase 14, v0.2 milestone).

`@fossil-lang/playground` v0.2 internal refactor — public API unchanged for v0.1.x consumers. Major changes:

- **IDE-style tabs layout** in `<FossilPlayground/>`: left panel `Mapping` / `Source` / `Shape`; right panel `Output` / `Compiled SQL`. Each panel is its own sub-component (`MappingPanel`, `SourcePanel`, `ShapePanel`, `OutputPanel`) consuming `@fossil-lang/ui` Tabs primitives.
- **Resizable horizontal split** between left/right via `<ResizablePanelGroup autoSaveId="fossil-playground-split">` — user-draggable, preference persists across reloads.
- **Toolbar extraction** — Run/Reset/Cite buttons composed with `@fossil-lang/ui` Tooltip primitives in a dedicated `<Toolbar/>` sub-component.
- **`useInferredDescriptors.introspectAndRegister(...)` awaited before compile** — closes the Phase 13 deferred follow-up so the InferredDescriptor flow (ADR-0037) actually fires on the playground's Run path (previously constructed but unused).
- **Folder reorg** — `run/introspection.ts` extracted; layered organization per CONTEXT.md.
- **Source tab** shows DuckDB DESCRIBE-inferred schemas; Shape tab is read-only via `extensions={[EditorView.editable.of(false)]}`.

Backwards compatibility: `<FossilPlayground/>` props + behavior unchanged for v0.1.x consumers. Tab switch resets CodeMirror view (scroll/cursor) — textual content survives via parent state (matches VS Code semantics; accepted partial per CONTEXT amendment iteration 1).

Bundle: 230.81 KB gzipped (PKG-03 cap 500 KB intact; +57 KB vs Phase 13 baseline driven by IDE Tabs + 4 panel sub-components + Resizable + Tooltip wiring). The CONTEXT.md aggressive ≤ 200 KB target missed by 30 KB; revisit in Phase 15 polish.
