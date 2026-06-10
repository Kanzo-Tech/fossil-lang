---
"@fossil-lang/executor": minor
---

Add `@fossil-lang/executor` — the DataFusion-WASM executor that runs fossil
mappings in the browser. Exposes `initFossilExecutor`, the `FossilExecutor`
class (`sources()` / `run()`), and the `runJob` end-to-end orchestration
(sources → signed-GET fetch → DataFusion-WASM → signed-PUT upload → complete)
with an injectable `JobTransport`.
