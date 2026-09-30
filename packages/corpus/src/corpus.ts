/**
 * A corpus, opened — **the door**, with the addressing underneath and invisible.
 *
 * One door over one manifest: `createGraphClient` is the transport it dispatches through and the
 * addressing is what it resolves with, and NEITHER is on the module surface. `/docs/design/one-door`
 * has what that settled, and `/docs/design/backend` the shape the door has: Apache Iceberg's read
 * path over OGC tile matrices.
 *
 * ```ts
 * const corpus = await open(url, { engine });
 * corpus.types                                   // what is inside
 * const set = corpus.tileMatrix('Person')        // every zoom, every tile, its rows and its box
 * const scan = corpus.scan({ type: 'Person', filter, select })
 * await scan.read(scan.plan().filter(visible))   // a batch per tile, a run in one statement
 * await corpus.edges({ from, direction: 'src' }) // the relations incident to those tiles
 * ```
 *
 * **The object has two halves and the line between them is not a spelling.** `tileMatrix`, `scan`
 * and `edges` compute which tiles to read and read those; `schema`, `relations` and
 * `executeSql` name a relation and let the engine decide. `fossil-graph`'s own crate doc states the
 * same rule from the other side: *pruning is which bytes are read, and that is the tiles' job, not
 * a verb's*.
 *
 * **The one thing a host brings is an engine** — `@fossil-lang/types`' `Engine`, which takes a
 * signal and answers in columns. This package links none, and decodes no Parquet.
 */

import type { Channel, Direction } from './address.js';
import type { EdgeAnswer, EdgesParams } from './edges.js';
import type {
  ExecuteSqlParams,
  ExecuteSqlResult,
  SchemaParams,
  SchemaResult,
} from './generated.js';
import type { Scan, ScanParams } from './scan.js';
import type { TileMatrixSet } from './tile-matrix.js';

/** One column of a vertex payload, as the bytes declare it. */
export interface CorpusField {
  readonly name: string;
  /** The engine's spelling of the Parquet type — `UINTEGER`, `VARCHAR`, `FLOAT`. */
  readonly type: string;
}

/** What one vertex type is: how much of it there is, and what each row carries. */
export interface CorpusVertexType {
  readonly type: string;
  /** Rows, from the manifest's `vertex_count`. */
  readonly count: bigint;
  readonly fields: readonly CorpusField[];
  /** The column an identity is read from, or `null` when the payload carries none. */
  readonly identity: string | null;
  /** Whether the payload carries `x` and `y`, without which a tile has no box. */
  readonly geometry: boolean;
  /**
   * What a view can draw the type with — the manifest's `channels:`, as declared. A cell tree's
   * `mode` is the mode of the one its `modeChannel` names, and a categorical's `domain` is the
   * value count a legend is drawn from.
   */
  readonly channels: readonly Channel[];
}

/** What one edge type is. `count` covers both orientations: they are one relation stored twice. */
export interface CorpusEdgeType {
  readonly edgeType: string;
  readonly srcType: string;
  readonly dstType: string;
  readonly count: bigint | null;
  /** The orientations the corpus publishes an address for — never one it does not. */
  readonly directions: readonly Direction[];
}

/** What is inside a corpus. */
export interface CorpusTypes {
  readonly vertices: readonly CorpusVertexType[];
  readonly edges: readonly CorpusEdgeType[];
}

/**
 * One relation the verbs register, as a host has to record it: the name the engine knows it by,
 * the rows it holds, the payload files behind it and — for a vertex type — the columns a row
 * carries. What {@link Corpus.relations} answers with.
 */
export type CorpusRelation =
  | {
      readonly kind: 'vertex';
      /** The vertex type, which is also its relation's name. */
      readonly name: string;
      /** The relation as SQL names it — qualified by the corpus's own catalog. */
      readonly sql: string;
      readonly rows: number;
      /** The payload — the projection at `scale: 1`. */
      readonly files: readonly string[];
      /** What a row carries, off the bytes — {@link CorpusVertexType.fields}. */
      readonly columns: readonly CorpusField[];
    }
  | {
      readonly kind: 'edge';
      /** `EdgeTypeSummary.table_name` — the corpus's own spelling, never composed by a host. */
      readonly name: string;
      /** The relation as SQL names it — qualified by the corpus's own catalog. */
      readonly sql: string;
      readonly rows: number;
      /** The source-aligned adjacency, which is the relation's rows. */
      readonly files: readonly string[];
      readonly edgeType: string;
      readonly srcType: string;
      readonly dstType: string;
    };

/** A corpus, open. */
export interface Corpus {
  /** Where it lives, as {@link open} was given it. */
  readonly url: string;
  /**
   * What is inside: the vertex and edge types, their counts, and every column a row carries.
   *
   * **Not {@link Corpus.schema}, and the difference is which artefact each one believes.** This is
   * read once while the corpus is opening — counts from the manifest's `vertex_count`, columns
   * from one `DESCRIBE` per type over the bytes — so it is a property rather than a call, it costs
   * nothing to read again, and it cannot go stale within an open corpus. `schema()` is a verb: it
   * asks the engine, counts with `count(*)`, takes its column list from the payload projection's
   * declared `properties`, and adds per-field cardinality and role for a type the call names.
   *
   * They disagree, and on the conformance corpus they disagree loudly: the manifest declares
   * THREE properties against seven columns on disk, so `schema()` speaks of `subject`,
   * `birth_year` and `postcode` while this reports those plus `dense_id`, `x`, `y` and
   * `cluster_id` — the four the writer puts there and the vocabulary does not name. That is not
   * one of them being wrong. A verb composes
   * SQL before it has seen a byte and can only read the declaration; this had a round trip to
   * spend and spent it on the artefact. Where the two must agree is a corpus guard's job —
   * `packages/corpus/guards`' `declared-count` is the one that catches a manifest lying about its rows.
   *
   * So: **this for what a row carries, `schema()` for what a field looks like.**
   */
  readonly types: CorpusTypes;
  /**
   * **One vertex type's tile matrices** — OGC's `TileMatrixSet`: its extent, and one matrix per
   * `z`, coarsest first, the payload last, each listing every tile with its rows and its box.
   *
   * Synchronous and total: it is the tile manifest `open` read, and no tile of any matrix is empty,
   * so a view picks its zoom — the finest whose visible tiles' rows fit its budget — without a
   * request.
   *
   * @throws {CorpusReadError} for a type that publishes no tile manifest.
   */
  tileMatrix(type: string): TileMatrixSet;
  /**
   * **Iceberg's `Table.scan`** — a type, a filter and a projection, bound when it is built, with
   * `plan()` over every zoom and `read(addresses)` of a batch per tile, a run of consecutive tiles in
   * one statement. See {@link Scan}.
   *
   * @throws {CorpusReadError} for a column no zoom carries, in the filter or the projection.
   */
  scan(params: ScanParams): Scan;
  /**
   * **The relations incident to each tile**, out of the half aligned on `direction` — the adjacency
   * at `z = Z`, the rung's quotient below it — and the ones declined, with fossil's `GapReason`: one
   * answer per address, a run of consecutive tiles read in one statement.
   */
  edges(params: EdgesParams): Promise<readonly EdgeAnswer[]>;
  // ── The verbs ─────────────────────────────────────────────────────────────────────────────
  //
  // Methods whose SQL is written in Rust — `fossil-graph` — and dispatched here through
  // `fossil-graph-wasm`. They were `createGraphClient`, a second entry point with no rule for
  // choosing between it and this one; it is the transport now, and this is the door.
  //
  // **They read a relation, where everything above reads tiles**, and that is the line between
  // the two halves of this object rather than a spelling difference. A verb's SQL names a table
  // and lets the engine decide which bytes to open; `scan` and `edges` compute which tiles
  // to read and read those. `crates/fossil-graph/src/lib.rs` states the same rule
  // from the other side — *pruning is which bytes are read, and that is the tiles' job, not a
  // verb's*.
  //
  // **What a verb sees is the manifest's vocabulary, not the payload's.** {@link open}
  // refuses to take the column list off the payload's declared `properties` and reads the bytes
  // instead, with the count that decided it; the verbs have no bytes at the time they compose SQL,
  // so they take the manifest at its word. On the conformance corpus that is three declared
  // properties against seven columns on disk, so `schema` lists `subject`, `birth_year` and
  // `postcode` while `types` reports all seven. Neither is wrong, and they are not the same —
  // see {@link Corpus.types}.

  /**
   * The type lists, and per-field statistics for a named `vertex_type`.
   *
   * See {@link Corpus.types} for which of the two believes the manifest and which believes the
   * bytes; they are not the same question and they disagree on this corpus.
   *
   * **`FieldStat.role === 'identifier'` is a chart-axis heuristic and nothing else.** The merge
   * puts it one call away from {@link CorpusVertexType.identity}, which is the column a subject
   * IRI is read from and the thing a bookmark keys on, and the two are unrelated: `role` guesses
   * what a column looks like so an axis can default sensibly, and it will call an integer `id`
   * column an identifier whether or not anything identifies anything with it. It is not a
   * governance classification, it does not mark a quasi-identifier, and nothing may gate a
   * disclosure decision on it. `identity` is the one that names an identity.
   */
  schema(params?: SchemaParams): Promise<SchemaResult>;
  /**
   * **Every relation the verbs query by name**, vertex types first, as a host records a corpus it
   * has to hand on: the name, the row count, the payload files and a vertex type's columns.
   *
   * The name is asked of {@link Corpus.schema} and never composed — `table_name` is pre-computed
   * there precisely so no binding reimplements the edge-naming convention — and the counts are its
   * `count(*)`, so this is one bare `schema()` and no query of its own. A relation that publishes
   * no source-aligned adjacency has no rows to register and is left out, as the verbs leave it.
   */
  relations(): Promise<readonly CorpusRelation[]>;
  /**
   * Give back what opening took from the engine: the catalog the verbs' views live in and, for a
   * job's corpus, the credential it was read with. Another open corpus of the same job on the
   * same engine keeps both until it closes too.
   */
  close(): Promise<void>;
}

/**
 * A corpus opened with `sql: 'allowed'` — {@link Corpus} plus the escape hatch.
 *
 * A second interface rather than an optional member, because the member is not optional: it is
 * present or it is not, and which one is decided at `open` by an argument the host wrote.
 * An `executeSql?:` would have made every caller of an OPEN corpus test for a member it knows it
 * has, and would have said nothing at all to a caller of a closed one.
 */
export interface SqlCorpus extends Corpus {
  /** The escape hatch: SQL over the views {@link Corpus.relations} names. */
  executeSql(params: ExecuteSqlParams): Promise<ExecuteSqlResult>;
}
