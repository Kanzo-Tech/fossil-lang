# @fossil-lang/profile

What the sources a Fossil program reads are — their columns and types, and unless a host asks for
types alone, how many rows they have, which columns are candidate keys, which columns hold another
source's keys, and a few sample rows — read on the host's DuckDB.

Which sources a program reads is fossil's answer, not this package's: `program.inputs()` walks the
AST and returns an `Input[]` with each `@conn/path` already expanded into a location and the
connection it goes through. This package asks the host for a read credential once per connection,
puts it in the host's DuckDB as a scoped secret (`@fossil-lang/storage`), runs `fossil-profile`'s
plan on that engine (`runProfiler` from `@fossil-lang/wasm`), and gives the credential back.

The plan — which queries describe a source, which pairs of columns to test, what each answer means —
is Rust, compiled into the wasm module, and it is the plan the language server runs on native DuckDB.
The two hosts share the code; only the engine differs.

```ts
import { profile } from "@fossil-lang/profile";

const profiled = await profile(await program.inputs(), {
  host,     // Host: vends a read credential per connection
  engine,   // Engine: DuckDB with httpfs, where the queries run
  options,  // optional: { depth: "types" } for the columns alone; the whole profile otherwise
  etag,     // optional: the source's ETag, so an unchanged source is not described again
  signal,   // optional: stops it
});
// the profile: profiled.profile — sources, inclusions, failures.
// both halves to the checker: the descriptors type the program, and each
// source in `undescribed` is a warning of `diagnostics` at its call, by its code.
program.registerProfile(profiled);
```

Best-effort: a source the host vends nothing for, or one DuckDB cannot describe, is skipped and
answered in `undescribed` with its problem — the editor degrades gracefully rather than
hard-failing. A materialised source (`io.rdf`) takes its schema from its shape and is not
described. A key or an inclusion read on a sample is a candidate, not a fact: `profile.sources[i]
.sampled` says which.
