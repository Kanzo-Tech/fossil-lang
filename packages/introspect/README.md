# @fossil-lang/introspect

Source schema introspection for the Fossil editor — the canonical TS home for
the logic that turns the sources a program reads into the `InferredDescriptor`s
the bidirectional checker (and editor field-completion) consume.

Which sources a program reads is fossil's answer, not this package's:
`program.inputs()` walks the AST and returns an `Input[]` with each `@conn/path`
already expanded into a location and the connection it goes through. This package asks the host for a read
credential once per connection, puts it in the host's DuckDB as a scoped secret
(`@fossil-lang/storage`), DESCRIBEs each source through the reader its
constructor names, gives the credential back, and builds the descriptor keyed by
the key the program wrote. It owns the DESCRIBE SQL, the DuckDB→Fossil primitive
table and the descriptor shape.

`crates/fossil-introspect` does the same job natively. Three things must agree
or a program means something different in the browser and in the language
server: the DuckDB reader each `io.` constructor picks (`read_csv_auto`,
`read_json_auto`, `read_parquet`), what DuckDB calls a reader option (`delim`),
and the DuckDB→primitive table. All three are `catalogue.bnf`'s, generated into
both sides by `cargo xtask catalogue`.

```ts
import { introspect } from "@fossil-lang/introspect";

const described = await introspect(await program.inputs(), {
  host,     // Host: vends a read credential per connection
  engine,   // Engine: DuckDB with httpfs, where the DESCRIBE runs
  etag,     // optional: the source's ETag, so an unchanged source is not described again
  signal,   // optional: stops it
});
// both halves to the checker: the descriptors type the program, and each
// source in `undescribed` is a warning of `diagnostics` at its call, by its code.
program.registerIntrospection(described);
```

Best-effort: a source the host vends nothing for, or one DuckDB cannot read, is
skipped and answered in `undescribed` with its problem — the editor degrades
gracefully rather than hard-failing. A materialised source (`io.rdf`) takes its schema from its
shape and is not described.
