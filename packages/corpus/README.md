# @fossil-lang/corpus

Open a `fossil/1` corpus and read it. **Fossil ships no viewer**; `@kanzo-tech/graph` is the one there is.

A corpus is a `fossil.json` and one Parquet file per table — one per vertex type, one per relation:

```
<dest>/
  fossil.json                             written last: it is the commit
  vertex/<Type>.parquet                   dense_id, subject, [x, y, cluster_id], the program's columns
  edge/<Src>_<label>_<Dst>.parquet        src, dst, the relation's columns
```

`dense_id` is global over the graph, gapless, and the drawn vertices come first. Every table is
written in the order of its key, in row groups of 122,880 rows, so DuckDB skips row groups by their
footer statistics.

## The door

```ts
import { open } from '@fossil-lang/corpus';

const corpus = await open(url, { engine });                  // a corpus at a URL
const corpus = await open(job, { engine, host });            // a job's, under the credential host vends
const corpus = await open(url, { engine, sql: 'allowed' });  // …with `sql`

corpus.manifest.vertex_tables     // name, path, key, identity, record_count, properties, position?
corpus.manifest.edge_tables       // name, label, path, source/destination {key, references}, record_count, properties

const scan = corpus.scan({ table: 'Person', select: ['dense_id', 'x', 'y'], filter: { bbox: [0, 0, 10, 10] } });
const batches = await scan.read(scan.plan(), { signal });   // Arrow tables, one per task

corpus.relation('Person')         // '"<url>"."Person"' — the view, as a statement of the host's must name it
corpus.url; corpus.schema         // the catalog and schema `information_schema` lists the views under

await corpus.sql('SUMMARIZE "<url>"."Person"');   // { columns, rows, truncated }, 10,000 rows by default
await corpus.close();
```

`open` reads `fossil.json` through the engine, refuses any `format` but `fossil/1` before it reads a
byte of Parquet, and creates one view per table — `"<url>"."Person"` — in a catalog named after the
corpus. A host's own SQL, Mosaic's included, reads the same relation `scan` does, and
`relation(table)` is that name — never quote it again.

**Each column the writer emits says what it IS**: `properties[].role` is `address` (the `key`),
`identity`, `coordinate`, `categorical` or `endpoint`, from `corpus.bnf`, and a program's column has
none. Hide the columns with a `role` from a field listing, colour by the `categorical` one — name the
role, never the column.

**It needs an engine, and the engine is the host's**: `@fossil-lang/types`' `Engine`, which answers
`query(sql, { signal })` in Arrow columns and interrupts the running statement when the signal
aborts. This package links no engine, decodes no Parquet and loads no WASM.

**`filter`** is data: `{ column, op, value }` with `= != < <= > >=`, `in`/`not in` over `values`,
`is null`/`not null`, `and`/`or`/`not`, and `{ bbox: [x0, y0, x1, y1] }` over the table's
`position`, closed on every edge. It is bound when the scan is built, so a column the table does not
declare fails before any statement runs.

**`plan()` is always one task today.** A table is one file and the engine prunes it; `plan()` is
the seam through which a table read in pieces would arrive without a caller changing.

## Reading a corpus without this package

`fossil.json` and `read_parquet` are the whole recipe — `/docs/format/reading/without-fossil`.

## The contract beside it

`guards/`, `conformance/` and `integration/` are the corpus contract in executable form, outside
`files`. `node guards/check.mjs <dir>` checks any corpus with `node` and the `duckdb` binary alone.
