# @fossil-lang/introspect

Source-binding schema introspection for the Fossil editor — the canonical TS
home for the logic that turns a `.fossil` mapping + its sources into
`InferredDescriptor`s the bidirectional checker (and editor field-completion)
consume.

Framework-agnostic, zero `@fossil-lang/*` runtime deps. The host injects the
**data plane** (URL resolution + a DuckDB executor); this package owns the
parsing, the DuckDB→Fossil primitive table, the DESCRIBE SQL, and the
descriptor shape — so the playground and keasy read one copy of it.

`crates/fossil-engine` does the same job natively, and the two are separate
implementations, not a shared one. Three things must agree or a program means
something different in the browser and on the CLI: the source-binding pattern,
the DuckDB reader each `io.` constructor picks (`read_csv_auto`,
`read_json_auto`, `read_parquet`), and the DuckDB→primitive table.
`tests/rust-parity.test.ts` reads that crate's source, derives all three from
it, and goes red when they diverge — which is the only reason this README is
allowed to say they agree. It says nothing about the rest of the two
implementations, which are free to differ and do.

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
