# @fossil-lang/introspect

Source schema introspection for the Fossil editor — the canonical TS home for
the logic that turns the sources a program reads into the `InferredDescriptor`s
the bidirectional checker (and editor field-completion) consume.

Which sources a program reads is fossil's answer, not this package's:
`FossilPlayground.sources(handle)` walks the AST and returns a
`ProgramSource[]` with each `@conn/path` already expanded into a locator. This
package signs every locator in one `SourceHost.sign` call, registers each
signed URL with the host's DuckDB under the key the program wrote, DESCRIBEs
it through the reader its constructor names, and builds the descriptor keyed by
that same key. It owns the DESCRIBE SQL, the DuckDB→Fossil primitive table and
the descriptor shape; it has no runtime dependency.

`crates/fossil-introspect` does the same job natively, and the two are separate
implementations, not a shared one. Three things must agree or a program means
something different in the browser and on the CLI: the DuckDB reader each `io.`
constructor picks (`read_csv_auto`, `read_json_auto`, `read_parquet`), what
DuckDB calls a reader option (`delim`), and the DuckDB→primitive table.
`tests/rust-parity.test.ts` reads that crate's source, derives all three from
it, and goes red when they diverge. The DESCRIBE statement itself is still
composed on both sides; making it one is a separate step.

```ts
import { introspect } from "@fossil-lang/introspect";

const descriptors = await introspect(playground.sources(handle), {
  host,     // SourceHost: signs locators
  engine,   // Engine: lends each signed URL as `sources/<key>` and runs the DESCRIBE
});
// host then registers each descriptor with the checker.
```

Best-effort: a source the host will not sign, or one DuckDB cannot read, is
reported through `onWarn` and skipped — the editor degrades gracefully rather
than hard-failing. A materialised source (`io.rdf`) takes its schema from its
shape and is not described.
