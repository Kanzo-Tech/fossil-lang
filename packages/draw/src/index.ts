/**
 * @fossil-lang/draw — the half of drawing a corpus that is not a renderer.
 *
 * ```ts
 * import { channelsFor, encodingFor, residency } from '@fossil-lang/draw';
 *
 * const encoding = encodingFor({ types: corpus.types, channels: channelsFor(manifests, 'Person') });
 * encoding.fill                      // what to colour by, declared or derived
 * encoding.columns                   // what a crossfilter view has to project
 *
 * const held = residency();
 * held.hold(key, await corpus.frame({ ...box, pixels }));
 * held.visible(nextBox, 20_000);     // an interim picture, out of answers already in hand
 * ```
 *
 * **It draws nothing.** There is no canvas, no context, no GPU and no frame loop anywhere in it,
 * and there is no camera: fossil still ships no viewer. What it holds is the two questions a
 * renderer has to ask before it can draw and that only the corpus can answer.
 *
 * - **What is it drawn WITH** — `./encoding.ts`. A vertex type declares a `channels:` block; this
 *   reads all three states of it (absent, empty, a list), ranks a declared channel above the
 *   privacy block's `quasi_identifiers`, ranks both above the bytes, and falls all the way back to
 *   a canvas of one colour rather than an exception.
 * - **What is already LOADED** — `./residency.ts`. The frames a door has already answered, an LRU
 *   over them, and the interim picture assembled out of whatever they hold while the settled answer
 *   for the next rectangle is in flight. `/docs/design/camera` names residency, visibility and the
 *   camera as three states a reader keeps apart; this is the first two, and it can be tested
 *   without the third.
 *
 * # Why this is a package and not `@fossil-lang/corpus`
 *
 * These two lived in `apps/playground/src/`, which is `private: true` and which no recursive CI
 * step reaches — so they shipped to nobody while `@kanzo-tech/graph` re-implemented the same seam
 * in 1,229 lines next door. Publishing them is the point. The question was only where.
 *
 * **Not on the door.** `@fossil-lang/corpus` was narrowed to seven value exports on *«there is no
 * second reference»*, and widening it back by five would have to be paid for by an argument that
 * these belong to the artefact's contract. They do not: a channel is read off a manifest but an
 * ENCODING is a reader's ranking of what to do with one, and a residency budget of 500,000 rows is
 * a number measured against one renderer on one corpus. A corpus does not have an opinion about
 * which of its columns deserves a histogram. That is exactly the difference between a contract and
 * a policy over it.
 *
 * **And the door costs a wasm module.** Every part of `@fossil-lang/corpus` static-imports
 * `fossil_graph_wasm.js` — `corpus.js`, `client.js`, `load.js` and `address.js` all do — so putting
 * a `channels:` line scanner behind that barrel makes instantiating a wasm module a precondition
 * for deciding which column is the colour. Nothing here needs one; nothing here needs an engine,
 * a request or a `query` either. `PAYLOAD_ADDRESS`, `PAYLOAD_COORDINATES` and `PAYLOAD_CATEGORICAL`
 * are the only values that cross the seam, and they are three string arrays off a generated table.
 *
 * **The rejected alternative is the one that was in force**: leave them in the app and let the
 * consumer that needs them write them again, which is the 1,229 lines this package exists to let
 * somebody delete. The second rejected alternative was a subpath per module on the door — rejected
 * because `@fossil-lang/corpus` already deleted `./address` for buying a "you need not load the
 * wasm" promise with a second implementation of the addressing, and a subpath whose justification
 * is what a caller can avoid loading is the same bargain in a new place.
 *
 * # What stayed in the app, and why the line is there
 *
 * `apps/playground/src/tiles.ts` and `whole.ts` both import `@kanzo-tech/graph`. They are the SEAM
 * — a translation between fossil's vocabulary and one renderer's contract — and a seam belongs to
 * whoever is on both sides of it, which is the app. Nothing in this package names a renderer, and
 * that is the test: if a module here ever needs to, it is in the wrong package.
 */

// What a corpus is drawn WITH. `Channel` is `@fossil-lang/types`' statement of the `channels:`
// wire shape, re-exported so a caller that reads a declaration and a caller that consumes one
// name the same type without either of them reaching for the executor.
export { categoricalOf, channelsFor, encodingFor, readChannels } from './encoding.js';
export type { Channel, Encoding, EncodingParams } from './encoding.js';

// What is already LOADED to draw it from.
export { residency } from './residency.js';
export type { Residency, Visible } from './residency.js';
