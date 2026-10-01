# @fossil-lang/introspect

Source schema introspection for the Fossil editor — the canonical TS home for
the logic that turns the sources a program reads into the `InferredDescriptor`s
the bidirectional checker (and editor field-completion) consume.

Which sources a program reads is fossil's answer, not this package's:
`FossilWorkspace.sources(handle)` walks the AST and returns a
`ProgramSource[]` with each `@conn/path` already expanded into a locator and
the connection it goes through. This package asks the host for a read
credential once per connection, puts it in the host's DuckDB as a scoped secret
(`@fossil-lang/storage`), DESCRIBEs each source through the reader its
constructor names, gives the credential back, and builds the descriptor keyed by
the key the program wrote. It owns the DESCRIBE SQL, the DuckDB→Fossil primitive
table and the descriptor shape.

`crates/fossil-introspect` does the same job natively, and the two are separate
implementations, not a shared one. Three things must agree or a program means
something different in the browser and in the language server: the DuckDB reader each `io.`
constructor picks (`read_csv_auto`, `read_json_auto`, `read_parquet`), what
DuckDB calls a reader option (`delim`), and the DuckDB→primitive table.
`tests/rust-parity.test.ts` reads that crate's source, derives all three from
it, and goes red when they diverge. The DESCRIBE statement itself is still
composed on both sides; making it one is a separate step.

```ts
import { introspect } from "@fossil-lang/introspect";

const { descriptors, undescribed } = await introspect(workspace.sources(handle), {
  host,     // Host: vends a read credential per connection
  engine,   // Engine: DuckDB with httpfs, where the DESCRIBE runs
  signal,   // optional: stops it
});
// host then registers each descriptor with the checker, and shows each
// `undescribed[i].problem` — the source it could not describe, by its code.
```

Best-effort: a source the host vends nothing for, or one DuckDB cannot read, is
skipped and answered in `undescribed` with its problem — the editor degrades
gracefully rather than hard-failing. A materialised source (`io.rdf`) takes its schema from its
shape and is not described.
