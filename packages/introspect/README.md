# @fossil-lang/introspect

Source-binding schema introspection for the Fossil editor — the canonical TS
home for the logic that turns a `.fossil` mapping + its sources into
`InferredDescriptor`s the bidirectional checker (and editor field-completion)
consume.

Framework-agnostic, zero `@fossil-lang/*` runtime deps. The host injects the
**data plane** (URL resolution + a DuckDB executor); this package owns the
parsing, the DuckDB→Fossil primitive table, the DESCRIBE SQL, and the
descriptor shape — so the playground, keasy, and the `fossil-cli` Rust sibling
stay in agreement instead of drifting across copies.

```ts
import { introspect } from "@fossil-lang/introspect";

const descriptors = await introspect(mappingText, {
  resolve: (ref) => signUrl(ref.url),        // host: ref → readable URL
  query: (sql) => duckdbConn.query(sql),     // host: run DESCRIBE → rows
});
// host then registers each descriptor with the editor / LSP worker.
```

Best-effort: a per-source failure (unreachable URL, DuckDB error) is logged and
skipped — the editor degrades gracefully rather than hard-failing.

See `.planning/EDITOR-SCHEMA-AWARE-PLAN.md` for the reference model.
