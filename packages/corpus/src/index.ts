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
 * import { openCorpus } from '@fossil-lang/corpus';
 * import wasmUrl from '@fossil-lang/corpus/pkg/fossil_graph_wasm_bg.wasm?url'; // Vite
 *
 * const corpus = await openCorpus(url, { query, wasmUrl });
 * corpus.types                                  // what is inside
 * await corpus.frame({ ...box, pixels })        // a rectangle at a resolution, ready to draw
 * await corpus.node(iri)
 * await corpus.aggregate({ vertex_type: 'Person', group_by: 'age', agg: 'count', bins: 20 })
 * ```
 *
 * # One door, and the rule applied to it
 *
 * *«There is no second reference»* is the repo's rule and this package has twice enforced it
 * against itself — `createGraphClient` is not exported, and the `./address` subpath was deleted
 * because its only justification forced a second implementation of the addressing. It had never
 * been applied to the door as a whole, which measured **10 value exports, 38 named types and an
 * unbounded `export type *`**. Three things came off it, each for its own reason:
 *
 * - **`initFossilGraphWasm`** — the biggest leak of implementation into a surface whose claim is
 *   that a corpus is a URL: a consumer had to know there is a wasm module and had to sequence two
 *   calls in the right order. {@link openCorpus} awaits it, and
 *   `OpenCorpusOptions.wasmUrl` is the one thing about it a caller can still need to say, because
 *   only the caller knows how its bundler resolves an asset.
 * - **`GRAPH_INFO_PATH`** — the index's file name is the door's business and not a consumer's;
 *   `openCorpus` takes a corpus URL and reads whatever is under it.
 * - **`export type *`** — an unbounded star publishes whatever the codegen makes, now and later,
 *   with nobody deciding. The fourteen the surviving surface names are re-exported below; the
 *   ten `./generated.ts` also holds (`Operation`, `FossilGraphSchemas`, the row and summary
 *   shapes) stay reachable structurally, e.g. `SchemaResult['vertices'][number]`.
 *
 * # What came back, and why narrowing is not the same as withdrawing
 *
 * **`resolveCorpus` came off beside `GRAPH_INFO_PATH` and is back.** The argument for taking it
 * off was that it and `corpus.addressing` answered one question from two routes, and the sentence
 * that reported the cost is the refutation: the door is the ENGINE-BEARING route and spends
 * `1 + N` round trips before it answers, `resolveCorpus` needs neither an engine nor a request,
 * and `corpus.addressing` is what the door already resolved for a caller who has paid for one.
 * Three positions, not one answer three times — and `apps/playground/src/bench.ts` names 245 tiles
 * of a million-vertex corpus from the position only the second one covers. Removing it removed a
 * capability. The rejected alternative is the one that was in force: leave it off and tell the two
 * callers that need it to re-implement `fossil_graph::plan`.
 *
 * **`Corpus.levels()` is `levelsOf` in `./address.ts`** — three calls to that module and no fourth
 * fact, which is why it is not a member of the door. It IS re-exported here, and was not: the
 * grounds were that a consumer never names a level file, and two do. See the export.
 *
 * **What deliberately stayed.** The four `PAYLOAD_*` role constants, because the check found a
 * consumer: `apps/playground/src/encoding.ts` reads all three of `PAYLOAD_ADDRESS`,
 * `PAYLOAD_COORDINATES` and `PAYLOAD_CATEGORICAL`, and `apps/playground/scripts/verify-encoding.mjs`
 * reads the last of them. Internalising them puts `cluster_id` back in a hand-written line in an
 * app — which is the six-statements-of-one-fact this table was generated to end.
 */

// The door, and the two errors an `instanceof` is a legitimate part of a surface for.
// `CorpusManifestError` is the manifest failing to address itself before a byte of payload is read;
// `CorpusReadError` is the bytes disagreeing with what the manifest promised. A caller can retry
// one of those against a different corpus and never the other.
export { CorpusManifestError, CorpusReadError, openCorpus } from './corpus.js';

// The OTHER door, and the reason there are two is that they answer different questions.
//
// **`resolveCorpus` was taken off this barrel as a duplicate of `corpus.addressing`, and it is not
// one.** `openCorpus` is the ENGINE-BEARING route: it takes a `query`, reads the index, then one
// manifest per type, and spends `1 + N` round trips before it hands anything back — because
// everything it answers needs bytes. `resolveCorpus` is the ENGINE-FREE route: hand it manifests
// you already hold and it names every URL the corpus can produce with no engine, no request and no
// round trip at all. `apps/playground/src/bench.ts` names all 245 tiles of a million-vertex corpus
// that way, and there is no expressing that through the door — which is what makes withdrawing
// this a withdrawn CAPABILITY rather than a removed duplicate.
//
// `corpus.addressing` is neither of those two things: it is what the door ALREADY resolved, for a
// caller that has one. It cannot be reached without paying for the door first, so it is not a
// second route to this answer; it is this answer, already bought.
//
// What genuinely was a second route stays internal. `initFossilGraphWasm` made a consumer know
// there IS a wasm module and order two calls against it; `wasmUrl` on `ResolveCorpusOptions` is the
// one thing about that boot a caller can still say, spelled exactly as it is on `OpenCorpusOptions`.
export { resolveCorpus } from './address.js';

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

// The one capability a host supplies, for every member of the door.
export type { QueryFn, QueryRow } from './query.js';

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
  OpenCorpusOptions,
  Pixels,
  PlacedEdge,
  PlacedVertex,
  RowsAnswer,
  RowsParams,
  SqlCorpus,
  SqlPolicy,
} from './corpus.js';

// What `Corpus.addressing` is, for the drawing path that fetches its own tiles. Named here because
// the member is: a public member whose type cannot be written down is worse than no member.
export type {
  AddressedTiles,
  CorpusAddressing,
  Direction,
  EdgeAddress,
  EdgeTiles,
  Gap,
  GapReason,
  IndexAddress,
  LevelInfo,
  ProjectionAddress,
  ResolveCorpusOptions,
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
