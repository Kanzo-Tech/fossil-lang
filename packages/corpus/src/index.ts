/**
 * @fossil-lang/corpus — open a corpus from a URL and read it.
 *
 * The query layer for whatever draws the graph — fossil ships no viewer. Verb→SQL runs in WASM
 * (`fossil-graph-wasm`, single-source with the native runtime), and SQL execution is delegated to a
 * host-provided DuckDB-WASM `query` callback (e.g. keasy's Mosaic coordinator). This is what makes
 * a viewer larger-than-RAM: the host's DuckDB streams Parquet over httpfs; the binding never
 * materialises rows in JS.
 *
 * ```ts
 * import { open } from '@fossil-lang/corpus';
 * import wasmUrl from '@fossil-lang/corpus/pkg/fossil_graph_wasm_bg.wasm?url'; // Vite
 *
 * const corpus = await open(url, { query, wasmUrl });
 * corpus.types                                  // what is inside
 * await corpus.frame({ ...box, pixels })        // a rectangle at a resolution, ready to draw
 * await corpus.node(iri)
 * await corpus.aggregate({ vertex_type: 'Person', group_by: 'age', agg: 'count', bins: 20 })
 * ```
 *
 * # One door, one name, and the depth is an argument
 *
 * *«There is no second reference»* is the repo's rule and this package has now enforced it against
 * itself three times — `createGraphClient` is not exported, the `./address` subpath was deleted
 * because its only justification forced a second implementation of the addressing, and
 * **`resolveCorpus` is gone into {@link open}**, which is the one that had survived two
 * previous passes.
 *
 * It survived them because the objection to removing it was real and is still real: it needed no
 * engine and spent no round trip, so deleting it withdrew a capability rather than a duplicate.
 * What that argument never established is that the capability needs a NAME of its own. It does not.
 * The capability is what the CALLER brings, so it is an argument:
 *
 * ```ts
 * await open(url,  { query, wasmUrl })          // Corpus — the door
 * await open(url,  { readText, wasmUrl })       // CorpusAddressing — manifests only
 * await open(base, { manifestFiles, wasmUrl })  // CorpusAddressing — no request at all
 * ```
 *
 * Three rungs, one name, and `corpus.addressing` is still what the first rung already resolved for
 * a caller who paid for it. What it cost: the call is now always asynchronous — the synchronous
 * form had no production consumer, measured, and both engine-free call sites in this repository
 * already awaited it.
 *
 * Also off the barrel, each for its own reason:
 *
 * - **`initFossilGraphWasm`** — the biggest leak of implementation into a surface whose claim is
 *   that a corpus is a URL: a consumer had to know there is a wasm module and had to sequence two
 *   calls in the right order. {@link open} awaits it, and
 *   `OpenOptions.wasmUrl` is the one thing about it a caller can still need to say, because
 *   only the caller knows how its bundler resolves an asset.
 * - **`GRAPH_INFO_PATH`** — the index's file name is the door's business and not a consumer's.
 *   **That reasoning did not extend to the engine-free route and it was applied there anyway**,
 *   which is what made a hand-written scan of the index's `vertices:`/`edges:` lists the price of
 *   addressing a corpus you had not already fetched — three copies of it, one of them in another
 *   repository. The file name stays off the surface and the SEQUENCE is published instead, as
 *   `OpenOptions.readText`: lend the package a text reader and it reads the index, the
 *   per-type manifests and nothing else. See {@link ReadTextFn}.
 * - **`export type *`** — an unbounded star publishes whatever the codegen makes, now and later,
 *   with nobody deciding. The fourteen the surviving surface names are re-exported below; the
 *   ten `./generated.ts` also holds (`Operation`, `FossilGraphSchemas`, the row and summary
 *   shapes) stay reachable structurally, e.g. `SchemaResult['vertices'][number]`.
 *
 * # What is here and why
 *
 * **`Corpus.levels()` is `levelsOf` in `./address.ts`** — three calls to that module and no fourth
 * fact, which is why it is not a member of the door. It IS re-exported here, and was not: the
 * grounds were that a consumer never names a level file, and two do. See the export.
 *
 * **The four `PAYLOAD_*` role constants**, because the check found a consumer:
 * `@fossil-lang/draw`'s `encoding.ts` reads all three of `PAYLOAD_ADDRESS`, `PAYLOAD_COORDINATES`
 * and `PAYLOAD_CATEGORICAL`, and `apps/playground/scripts/verify-encoding.mjs` reads the last of
 * them through it. Internalising them puts `cluster_id` back in a hand-written line — which is the
 * six-statements-of-one-fact this table was generated to end. That consumer was in an app when this
 * was written and is a published package now, which makes the reason stronger rather than weaker.
 */

// The door — one name, three depths — and the two errors an `instanceof` is a legitimate part of a
// surface for. `CorpusManifestError` is the manifest failing to address itself before a byte of
// payload is read; `CorpusReadError` is the bytes disagreeing with what the manifest promised. A
// caller can retry one of those against a different corpus and never the other.
export { CorpusManifestError, CorpusReadError, open } from './corpus.js';

// Which levels of detail a type has, and which of them the writer spent bytes on.
//
// It came off `Corpus` in the same change and did not leave the tree — every line of it is
// addressing. It is back on the barrel because two readers outside this package name a level file:
// `apps/playground/scripts/measure-frame.mjs` and `measure-pyramid.mjs` report the written levels
// beside what a frame cost. `Frame.matchedAt` is a member of the door and reports a level; what
// makes that number readable has to be reachable from the same surface.
export { levelsOf } from './address.js';

// The writer's column table, by ROLE — `corpus.bnf` through `cargo xtask corpus`, and the same
// table `crates/fossil-sinks/src/generated.rs` carries on the Rust side.
//
// Exported because the alternative is a consumer spelling the names again. `cluster_id` was
// written down in `corpus.bnf`, in this package's generated file, twice in this package, and three
// times in `apps/playground` — six statements of one fact, of which the only machine-readable one
// had no consumer. A reader asks for the ROLE it means and gets whatever the writer calls it.
//
// **All four and not the three with a consumer today.** They are one generated table with one
// membership decision, and publishing a subset of it by who imports what this week is a fifth place
// that decision gets taken. `ColumnRole` and `WriterColumn` are NOT here: they describe
// `PAYLOAD_COLUMNS`, which is not exported.
export {
  PAYLOAD_ADDRESS,
  PAYLOAD_CATEGORICAL,
  PAYLOAD_COORDINATES,
  PAYLOAD_IDENTITY,
} from './vocabulary.generated.js';

// The capabilities a host supplies — the engine for every member of the door, and the text reader
// for the rung that has no engine to lend. See `./query.ts` for why the second exists at all.
export type { QueryFn, QueryRow, ReadTextFn } from './query.js';

// The door's own types. `SqlCorpus` is what `sql: 'allowed'` widens the answer to — see
// `SqlPolicy` for why one option decides both raw-SQL doors, and `crates/fossil-mcp/src/tools.rs`
// for the native surface this is the port of.
export type {
  Answer,
  Box,
  Corpus,
  CorpusEdgeType,
  CorpusField,
  CorpusTypes,
  CorpusVertexType,
  Extent,
  Frame,
  FrameCost,
  FrameParams,
  Neighbourhood,
  NeighboursParams,
  NodeParams,
  OpenOptions,
  Pixels,
  PlacedEdge,
  PlacedVertex,
  RowsAnswer,
  RowsParams,
  SqlCorpus,
  SqlPolicy,
} from './corpus.js';

// What `Corpus.addressing` is — and, since the collapse, what the two engine-free rungs of
// `open` answer with. Named here because the member is: a public member whose type cannot be
// written down is worse than no member.
//
// **`Container` is on this list and was not, which was the same omission one layer down.**
// `CorpusAddressing.container`, `VertexAddress.container`, `IndexAddress.container` and
// `ProjectionAddress.container` are all public members of that type, and a consumer could read the
// discriminant and not name it. It carries the rule a footer reader needs — under `rowgroups` the
// row group IS the tile, under `files` the file is — which lived in `/docs/format` prose and now
// lives on the type, where the reader that needs it already is.
//
// `ResolveCorpusOptions` is NOT replaced by another name: `OpenOptions` is what it became.
export type {
  AddressedTiles,
  Container,
  CorpusAddressing,
  Direction,
  EdgeAddress,
  EdgeTiles,
  Gap,
  GapReason,
  IndexAddress,
  LevelInfo,
  ProjectionAddress,
  VertexAddress,
} from './address.js';

// The twelve the verbs name in their own signatures, plus the two `Corpus.schema` argues about by
// name — from the schemars codegen, single source of truth with the Rust verb structs. This list
// replaces `export type *`: a name reaches a consumer because the surviving surface mentions it,
// and for no other reason.
//
// **`FieldStat` and `FieldRole` are the second kind and they are not speculative.** Keasy's
// `web/src/lib/graph-schema.ts` imports both today, and `Corpus.schema`'s own doc spends a
// paragraph on `FieldStat.role === 'identifier'` being a chart-axis heuristic and NOT an identity
// — a warning a consumer cannot act on if it cannot name the type it is about. The other ten
// `./generated.ts` holds (`Operation`, `FossilGraphSchemas`, the row and summary shapes) stay
// reachable structurally, e.g. `SchemaResult['vertices'][number]`.
export type {
  AggregateParams,
  AggregateResult,
  ExecuteSqlParams,
  ExecuteSqlResult,
  ExpandParams,
  ExpandResult,
  FieldRole,
  FieldStat,
  PathParams,
  PathResult,
  ReadParams,
  ReadResult,
  SchemaParams,
  SchemaResult,
} from './generated.js';
