# @fossil-lang/corpus

Attach a `fossil/1` corpus to your DuckDB and read it with SQL. **Fossil ships no viewer**;
`@kanzo-tech/graph` is the one there is.

A corpus is a `fossil.json` and one Parquet file per table — one per vertex type, one per relation:

```
<dest>/
  fossil.json                             written last: it is the commit
  vertex/<Type>.parquet                   dense_id, subject, the program's columns
  edge/<Src>_<label>_<Dst>.parquet        src, dst, the relation's columns
```

`dense_id` is global over the graph and gapless: the types in manifest order, each one contiguous
range, subject order inside it. Every table is written in the order of its key, in row groups of
122,880 rows, so DuckDB skips row groups by their footer statistics. The corpus carries the graph,
not a picture: where a vertex is drawn is the view's choice.

## The door

```ts
import { open } from '@fossil-lang/corpus';

const close = await open(job, { engine, host });           // a job's corpus, under the credential host vends
const close = await open('demo', { engine, url });         // a corpus at a URL

// everything else is SQL, over the catalog named by the first argument:
//   "<name>"."Person"            a view per table
//   "<name>".fossil_tables       table_name, kind, iri, path, rows, first_id, source, destination
//   "<name>".fossil_columns      table_name, column_name, ordinal, type, role, iri, nullable

await close();
```

`open` reads `fossil.json` through the engine, refuses any `format` but `fossil/1` before it reads a
byte of Parquet, creates one view per table and the manifest as two relations, and answers the
function that detaches them. Two opens of one name share the catalog; the last to close detaches it.

**`fossil_tables`** is one row per table in manifest order. A vertex table's ids are
`first_id … first_id + rows − 1`; an edge table's `source` and `destination` name the vertex tables
its `src` and `dst` point into.

**`fossil_columns`** says what each column the writer emits IS: `role` is `address` (`dense_id`),
`identity` (`subject`) or `endpoint` (`src`, `dst`), from `corpus.bnf`, and `NULL` on a program's
column. Hide the columns with a role from a field listing — name the role, never the column.

**It needs an engine, and the engine is the host's**: `@fossil-lang/types`' `Engine`, which answers
`query(sql, { signal })` in Arrow columns. This package links no engine, decodes no Parquet and loads
no WASM.

## Its RDF meaning

```ts
import { mapping } from '@fossil-lang/corpus';

const turtle = mapping(fossilJsonText);   // an R2RML mapping, in Turtle
```

`mapping` is a pure function of `fossil.json`: each vertex type a triples map over its table (the
`subject` as an IRI, the type's class, a literal per column with an IRI), each relation a triples map
over an `rr:sqlQuery` in Core SQL 2008 that joins `src`/`dst` to the `dense_id` of the tables they
reference and takes their subjects. What has no IRI is not mapped. Every name is a delimited
identifier, unqualified: resolve it against the database you attached the corpus to. Any R2RML
processor then reads the corpus as RDF — `/docs/format/reading/rdf`.

## Reading a corpus without this package

`fossil.json` and `read_parquet` are the whole recipe — `/docs/format/reading/without-fossil`.

## The contract beside it

`guards/`, `conformance/` and `integration/` are the corpus contract in executable form, outside
`files`. `node guards/check.mjs <dir>` checks any corpus with `node` and the `duckdb` binary alone.
