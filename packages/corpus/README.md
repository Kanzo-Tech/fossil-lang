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
import { attach } from '@fossil-lang/corpus';

await using corpus = await attach(job, { engine, host });     // a job's corpus, under the credential host vends
const other = await attach('demo', { engine, url });          // a corpus at a URL

// everything else is SQL, over the catalog named by the first argument:
//   "<name>"."Person"            a view per table
//   "<name>".fossil_tables       table_name, kind, iri, path, record_count, first_id, source, destination, derived_from
//   "<name>".fossil_columns      table_name, column_name, ordinal_position, data_type, role, iri, datatype, is_nullable
//   "<name>".triples             s_type, s_value, p, o_type, o_value, o_datatype, o_lang — the corpus as RDF

await other.detach();             // `await using` detaches `corpus` when the block ends
```

`attach` reads `fossil.json` through the engine, refuses any `format` but `fossil/1` before it reads a
byte of Parquet, creates one view per table, the manifest as two relations and the corpus as triples, and
answers an `Attachment` — `name`, `detach()` and `[Symbol.asyncDispose]` — that detaches them. Two
attachments of one name share the catalog; the last to detach takes it.

**`fossil_tables`** is one row per table in manifest order. A vertex table's ids are
`first_id … first_id + record_count − 1`; an edge table's `source` and `destination` name the vertex
tables its `src` and `dst` point into; `derived_from` is the sources the table was derived from, each
as the program wrote it.

**`fossil_columns`** says what each column the writer emits IS: `role` is `address` (`dense_id`),
`identity` (`subject`) or `endpoint` (`src`, `dst`), from `corpus.bnf`, and `NULL` on a program's
column. Hide the columns with a role from a field listing — name the role, never the column.

**It needs an engine, and the engine is the host's**: `@fossil-lang/types`' `Engine`, which answers
`query(sql, { signal })` in Arrow columns. This package links no engine, decodes no Parquet and loads
no WASM.

## Its RDF meaning

`"<name>".triples` is the corpus as RDF, a row per triple: each vertex's `subject` as an IRI with its
type's class, a literal per column with an IRI — the term its shape declared, else the natural one of
R2RML §10.2 — and each relation's two subjects, joined through `src`/`dst` and the `dense_id` of the
tables they reference. What has no IRI is not mapped. A SHACL engine validates it as it stands:

```ts
const report = await shapes.validateTable({ table: `"${job}".triples`, engine });   // rudof
```

The columns and the rules are `/docs/format/reading/rdf`.

## Reading a corpus without this package

`fossil.json` and `read_parquet` are the whole recipe — `/docs/format/reading/without-fossil`.

## The contract beside it

`guards/`, `conformance/` and `integration/` are the corpus contract in executable form, outside
`files`. `node guards/check.mjs <dir>` checks any corpus with `node` and the `duckdb` binary alone.
