/**
 * A corpus, opened from a URL — **the door**, with the addressing underneath and invisible.
 *
 * There were three entry points over one manifest and no rule for choosing between them, and this
 * is the one door now: `createGraphClient` is the transport it dispatches through, `addressManifests`
 * is the addressing it resolves with, and NEITHER is on the module surface — a consumer of
 * `@fossil-lang/corpus` reaches both through this function or not at all. `./index.ts` carries what
 * each internalisation cost. `/docs/design/one-door` has what the removal settled, and why the
 * camera grew this object rather than opening a fourth beside it.
 *
 * **`resolveCorpus` was the last of the three, and it is `corpus.addressing` now.** The addressing
 * is what every open resolves first, and a caller who wants the URLs reads them off the corpus it
 * opened — there is no engine-free door beside it. See {@link open} and {@link OpenOptions}.
 *
 * **The object has two halves and the line between them is not a spelling.** `extent`, `rows`,
 * `node` and `neighbours` compute which FILES to open and open those; `schema`, `relations` and
 * `executeSql` name a relation and let the engine decide. The camera is
 * addressed, not queried — an LOD is a different relation and not a filter — and `fossil-graph`'s
 * own crate doc states the same rule from the other side: *pruning is which bytes are read, and
 * that is the tiles' job, not a verb's*. See {@link Corpus.neighbours} for the one place the two
 * halves answer questions that look identical and are not.
 *
 * The addressing is not this. It returns URLs and leaves the consumer knowing what a tile is,
 * which container carries one, how to ask for footers and how to join CSR with CSC. That is exactly
 * the knowledge the handover asked not to need. Here — with an engine given — there are no tiles,
 * no `dense_id`, no Morton, no `by_source`, no prefixes and no footers in the caller's face:
 *
 * ```ts
 * const corpus = await open(url, { query });
 * corpus.types                                  // what is inside
 * await corpus.rows({ x, y, w, h })             // vertices + edges, and whether that is all of them
 * await corpus.node(iri)
 * await corpus.neighbours([iri], { depth: 2 })
 * await corpus.schema({ vertex_type: 'Person' })
 * ```
 *
 * **The one thing a host brings is an engine.** See `./query.ts` for why the capability is a single
 * `query` callback and not a bundled Parquet decoder: this package still has zero runtime
 * dependencies, and the engine is the one the host already has.
 *
 * That sentence used to end «and DuckDB's own footer pruning is the half of a windowed read nobody
 * has to write», which is true of DuckDB and false as a reason to prefer it. Measured on a
 * million-vertex corpus over HTTP: on the windowed read both DuckDB v1.5.3 and DataFusion 54 open
 * exactly **5 of 245** row groups, and DataFusion reads **2.6× fewer bytes** doing it, because
 * DuckDB's httpfs floors every footer read at 16 KiB. The pruning is not a differentiator. What
 * this package actually buys by taking a callback is that it links no engine at all.
 *
 * **Five things a consumer used to supply out of its own head.** Four are absorbed and the fifth is
 * declared, and each is argued where it bites rather than here:
 *
 * - **How many tiles** — from the manifest. `vertex_count` and `chunk_size` are both required
 *   fields, so `ceil(count / chunk_size)` settles it in one `read_text`. Before that there was no
 *   way: HTTP gives no directory, and the written alternative was to probe with `HEAD` until a 404.
 *   A corpus that declares no count is the one this refuses to open.
 * - **The payload vocabulary** — from the bytes, with one `DESCRIBE` per vertex type, and
 *   deliberately not from the payload projection's declared `properties`. See {@link open}
 *   for the count that decided it.
 * - **The `x`/`y` boxes** — from the Parquet footers, as {@link Corpus.extent}, which is a fifth
 *   member on a surface that names four because without it a caller holding only a URL has no
 *   coordinates to put in a rectangle.
 * - **Corpus identity** — the subject IRI, and this was the one with no owner. See
 *   {@link Corpus.node}: what it costs, what the tree contradicts itself about, and what would
 *   change it.
 * - **Which container** — from the manifest's `container`, because a reader over HTTP has no
 *   directory to list. Both are read; neither is globbed. See {@link open}.
 *
 * And one shape of corpus it refuses rather than guesses at: **an edge label incident twice to one
 * vertex type**, which the addressing's `tilesFor` cannot name unambiguously. See {@link Corpus.rows}.
 */

import type { Channel, CorpusAddressing, Direction } from './address.js';
import type { Neighbourhood, NeighboursParams } from './edges.js';
import type { Frame, FrameParams } from './frame.js';
import type {
  ExecuteSqlParams,
  ExecuteSqlResult,
  SchemaParams,
  SchemaResult,
} from './generated.js';
import type { NodeParams, PlacedVertex } from './identity.js';
import type { RowsAnswer, RowsParams } from './scan.js';
import type { Extent } from './tile-manifest.js';
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
  /** Whether the payload carries `x` and `y`, without which no box can be answered. */
  readonly geometry: boolean;
  /**
   * Whether a lookup by identity on this type is a **seek** or a **scan**.
   *
   * `true` when the corpus publishes an identity index: {@link Corpus.node} and
   * {@link Corpus.neighbours}'s seed resolution read one index tile and then the payload tiles
   * those addresses name. `false` when it does not: the same call reads the identity column of
   * every tile of the type, which at five million vertices is about 40 MB.
   *
   * It is a property of the TYPE and not of a call, because that is the shape of the fact: a
   * consumer needs to know once whether its bookmarks are cheap, not to be told again on every
   * click. Both answers are correct; only one of them is fast, and a caller with no way to tell
   * them apart discovers the difference by measuring.
   */
  readonly indexed: boolean;
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
   * The addressing underneath, for a caller that has outgrown this surface — a drawing path that
   * wants tile URLs to fetch itself, for instance. Nothing here needs it.
   */
  readonly addressing: CorpusAddressing;
  /**
   * The bounding box of one vertex type's positions, or `null` when it has no geometry.
   *
   * **It comes from the Parquet footers**, which is one request per tile and then nothing: the same
   * row-group statistics that let the engine skip tiles a window misses. It is therefore *believed*
   * rather than verified — a box wider than its rows costs a read, a box narrower than its rows
   * loses vertices, and only re-reading every page tells them apart. Cached after the first call.
   *
   * It is a fifth member on a specification that names four, and it is here because without it
   * {@link Corpus.rows} cannot be called: a caller with a URL and nothing else has no coordinates
   * to put in the box.
   */
  extent(type?: string): Promise<Extent | null>;
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
  /** The vertices in a rectangle and the edges among them, entire. */
  rows(params: RowsParams): Promise<RowsAnswer>;
  /**
   * A rectangle at a resolution, ready to draw — **the door a camera goes through.**
   *
   * **This and {@link Corpus.rows} both take a rectangle and they are not the same question.**
   * `rows` answers *what is here*: every row, every column, complete for incidence, no bound. This
   * answers *what do I draw at this resolution*: a decimation, positions and one categorical,
   * bounded by the level rather than by a cap. What keeps them one contract rather than two is
   * that **a frame at level 0 selects the same vertices `rows` does over the same rectangle**,
   * which `tests/frame.test.ts` asserts against the fixture.
   *
   * **Two names rather than one call whose return type moved with a flag**, which is what these
   * two were. The level comes from {@link FrameParams.pixels} — the pyramid is complete, from
   * level 1 to the level that fits one tile, so the derived level is always answerable and the
   * second call that used to name it is gone. {@link FrameParams.level} still says one outright.
   */
  frame(params: FrameParams): Promise<Frame>;
  /** One vertex by identity, or `null`. */
  node(id: string, params?: NodeParams): Promise<PlacedVertex | null>;
  /**
   * Everything within `depth` hops of a set of identities.
   *
   * It turns the frontier into tile numbers by shifting the addresses it already holds, opens those
   * adjacency tiles and no others, and does it once per hop — where a recursive CTE over the whole
   * edge relation did not return in 45 seconds at a million vertices. It answers with placements,
   * and admits {@link Answer.complete}, {@link Answer.gaps} and {@link Neighbourhood.frontier}: the
   * orientations not read and the boundary the depth bound cut, because an answer whose outermost
   * ring is missing its own edges looks whole in a count.
   */
  neighbours(ids: Iterable<string>, params?: NeighboursParams): Promise<Neighbourhood>;

  // ── The verbs ─────────────────────────────────────────────────────────────────────────────
  //
  // Methods whose SQL is written in Rust — `fossil-graph` — and dispatched here through
  // `fossil-graph-wasm`. They were `createGraphClient`, a second entry point with no rule for
  // choosing between it and this one; it is the transport now, and this is the door.
  //
  // **They read a relation, where everything above reads tiles**, and that is the line between
  // the two halves of this object rather than a spelling difference. A verb's SQL names a table
  // and lets the engine decide which bytes to open; `rows`, `extent` and `neighbours` compute
  // which files to open and open those. `crates/fossil-graph/src/lib.rs` states the same rule
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
