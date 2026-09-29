/**
 * @fossil-lang/corpus — open a corpus from a URL and read it.
 *
 * The query layer for whatever draws the graph — fossil ships no viewer. Verb→SQL runs in WASM
 * (`fossil-graph-wasm`, single-source with the Rust verb structs), and SQL execution is delegated to a
 * host-provided DuckDB-WASM `query` callback (e.g. keasy's Mosaic coordinator). This is what makes
 * a viewer larger-than-RAM: the host's DuckDB streams Parquet over httpfs; the binding never
 * materialises rows in JS.
 *
 * ```ts
 * import { open } from '@fossil-lang/corpus';
 *
 * const corpus = await open(url, { query });
 * corpus.types                                  // what is inside
 * await corpus.frame({ ...box, pixels })        // a rectangle at a resolution, ready to draw
 * await corpus.node(iri)
 * ```
 *
 * # One door, one name, and the depth is an argument
 *
 * *«There is no second reference»* is the repo's rule and this package has now enforced it against
 * itself three times — `createGraphClient` is not exported, the `./address` subpath was deleted
 * because its only justification forced a second implementation of the addressing, and
 * **`resolveCorpus` is gone into {@link open}**, whose engine-free rungs went with the viewer that
 * was their only consumer:
 *
 * ```ts
 * await open(job, { engine, host })  // a job's corpus
 * await open(url, { query })         // a corpus at a URL
 * ```
 *
 * `corpus.addressing` is what the open already resolved, for a caller who wants the URLs.
 *
 * Also off the barrel, each for its own reason:
 *
 * - **`initFossilGraphWasm`** — the biggest leak of implementation into a surface whose claim is
 *   that a corpus is a URL: a consumer had to know there is a wasm module and had to sequence two
 *   calls in the right order. {@link open} awaits it, and the module finds its own `.wasm`
 *   through `new URL(…, import.meta.url)`, which the host's bundler emits as an asset.
 *   `OpenOptions.wasm` is left for the host with no bundler (Node: the bytes).
 * - **`GRAPH_INFO_PATH`** — the index's file name is the door's business and not a consumer's:
 *   every open reads the index and the manifests it names through the engine it was given.
 * - **`export type *`** — an unbounded star publishes whatever the codegen makes, now and later,
 *   with nobody deciding. The seventeen the surviving surface names are re-exported below; the
 *   rest of `./generated.ts` (`Operation`, `FossilGraphSchemas`, the verb row
 *   shapes) stay reachable structurally, e.g. `SchemaResult['vertices'][number]`.
 *
 * # What is here and why
 *
 * **`Corpus.levels()` is `levelsOf` in `./address.ts`** — three calls to that module and no fourth
 * fact, which is why it is not a member of the door. It is not on the barrel either: the two
 * readers outside this package that named a level file were verifier scripts in a viewer this
 * repository no longer has.
 *
 * **The four `PAYLOAD_*` role constants**, because a consumer reads them: `@kanzo-tech/graph`
 * takes `PAYLOAD_ADDRESS`, `PAYLOAD_COORDINATES` and `PAYLOAD_IDENTITY` rather than spelling the
 * writer's columns again. Internalising them puts `cluster_id` back in a hand-written line — which
 * is the six-statements-of-one-fact this table was generated to end.
 */

// The door — one name, three depths — and the two errors an `instanceof` is a legitimate part of a
// surface for. `CorpusManifestError` is the manifest failing to address itself before a byte of
// payload is read; `CorpusReadError` is the bytes disagreeing with what the manifest promised. A
// caller can retry one of those against a different corpus and never the other.
export { CorpusManifestError, CorpusReadError, open } from './corpus.js';

// The writer's column table, by ROLE — `corpus.bnf` through `cargo xtask corpus`, and the same
// table `crates/fossil-sinks/src/generated.rs` carries on the Rust side.
//
// Exported because the alternative is a consumer spelling the names again. `cluster_id` was
// written down in `corpus.bnf`, in this package's generated file, twice in this package, and three
// times in a viewer app — six statements of one fact, of which the only machine-readable one
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

// The capability a host supplies — the engine every member of the door reads through.
export type { QueryFn, QueryRow } from './query.js';

// The door's own types. `SqlCorpus` is what `sql: 'allowed'` widens the answer to — see
// `SqlPolicy` for what the option decides.
//
// **`OpenOptions` is the one exported type with no `Corpus` in its name, and that is decided
// rather than overlooked.** The prefix on every other name here is doing real work — `Corpus`,
// `CorpusTypes`, `CorpusField`, `CorpusAddressing` all name the ARTEFACT or a part of it, and a
// consumer holds them. This one names the CALL: it appears in exactly one position, the second
// argument of `open`, never in a return type and never in a field, and an options bag whose name
// does not track its function is how `ResolveCorpusOptions` outlived `resolveCorpus` in another
// repository's import list. The type follows the function — that is the rule, and it is the same
// rule that renamed `ResolveCorpusOptions` when `resolveCorpus` was folded in.
//
// It is also the spelling that costs a consumer least, measured rather than supposed:
// `@kanzo-tech/graph` exports an `OpenCorpusOptions` of its own for its own opener, so keeping
// ours under that name would have forced an import alias in the one external file that binds this
// door. **Do not "restore" the prefix.**
export type {
  Answer,
  Box,
  Corpus,
  CorpusEdgeType,
  CorpusField,
  CorpusRelation,
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

// What `Corpus.addressing` is. Named here because the member is: a public member whose type cannot be
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
  Drawing,
  EdgeAddress,
  EdgeTiles,
  Gap,
  GapReason,
  IndexAddress,
  ProjectionAddress,
  VertexAddress,
} from './address.js';

// The four the verbs name in their own signatures, plus the five a reader of `Corpus.schema`
// names — from the schemars codegen, single source of truth with the Rust verb structs. This list
// replaces `export type *`: a name reaches a consumer because the surviving surface mentions it,
// and for no other reason.
//
// **`FieldStat` and `FieldRole` are the second kind and they are not speculative.** Keasy's
// `web/src/lib/graph-schema.ts` imports both today, and `Corpus.schema`'s own doc spends a
// paragraph on `FieldStat.role === 'identifier'` being a chart-axis heuristic and NOT an identity
// — a warning a consumer cannot act on if it cannot name the type it is about. `FieldKind` is the
// same argument: it is what a binnable or temporal axis is decided by, and a host that cannot name
// it writes the GraphAr spelling table again. `VertexTypeSummary` and `EdgeTypeSummary` are the
// rows of `SchemaResult` a schema panel is drawn from, and naming them structurally was the price
// of a host keeping its own copy. The rest of `./generated.ts` (`Operation`,
// `FossilGraphSchemas`, the verb row shapes) stays reachable structurally.
export type {
  ExecuteSqlParams,
  ExecuteSqlResult,
  EdgeTypeSummary,
  FieldKind,
  FieldRole,
  FieldStat,
  SchemaParams,
  SchemaResult,
  VertexTypeSummary,
} from './generated.js';
