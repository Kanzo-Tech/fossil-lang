/**
 * What a vertex type's rows are **drawn with** — the `channels:` block of a vertex manifest.
 *
 * `crates/fossil-sinks/src/manifest.rs, Channel` is the writer and this is the wire shape of it in
 * TypeScript. There is one such statement and this is it.
 *
 * ## Why here, and not beside the rest of that manifest
 *
 * The other documents of a corpus manifest — `GraphInfo`, `VertexInfo`, `CoordinateSystem`,
 * `EdgeInfo` — are mirrored in `@fossil-lang/executor`, because the executor is what HANDS THEM
 * BACK: a run produces the files and the manifests that address them, and the shapes belong beside
 * the thing that returns them. A channel is the one entry of that set with a second reader.
 * `@fossil-lang/draw` reads a `channels:` block off a manifest it was handed and turns it into an
 * encoding, and it does that having run nothing — no mapping, no DataFusion, no wasm.
 *
 * So the alternative was measured rather than argued: leaving it in `@fossil-lang/executor` makes
 * every consumer of a drawing policy install 22 MB of `fossil_df_wasm` to name one interface, and
 * restating it in the reader makes three TypeScript statements of a Rust struct — which is exactly
 * what the reader that consumes it was written to stop doing. This package is the family's
 * zero-runtime shared-type home and both readers can reach it, so the shape sits where both are.
 * `@fossil-lang/executor` re-exports it, so `VertexInfo.channels` and every import that already
 * named it are unchanged.
 */

/**
 * One channel over a vertex type's rows: a column, the scale it is read on, and — where a reader
 * cannot recover it — the size of its domain.
 *
 * `crates/fossil-sinks/src/manifest.rs, CoordinateSystem` is the precedent and this copies it
 * rather than re-deriving it: a flat mapping, a list on the type, three states. The two answer
 * different questions about the same rows — that one is *where a row is*, this one is *what it
 * looks like there*.
 */
export interface Channel {
  /** The writer's name for the channel. Nothing reserves any. */
  name: string;
  /** The payload column it is read out of. */
  column: string;
  scale: Scale;
  /**
   * How many distinct values a **categorical** has.
   *
   * The one number a reader cannot recover: a distinct count is in no Parquet footer, which is the
   * same test `HolonTree`'s `vertices_per_cell` passes. Absent on a quantitative channel, whose
   * domain is a range — and the range is already in the footers' per-row-group min/max, so
   * declaring it would be a second statement of what the bytes carry.
   *
   * Not `cardinality`: `Property` in `@fossil-lang/executor` does not carry that field, but the
   * manifest's Rust `Property` does and it means SHACL multiplicity. Two fields a spelling apart in
   * one document is how a reader comes to answer one with the other.
   */
  domain?: number;
  /** What computed the column, present exactly when the writer did. Absent ⇒ it came off the source. */
  derived_by?: string;
}

/**
 * How a channel's column is read — a closed set, because a reader **dispatches** on it: a
 * categorical is a lookup into a palette of finite capacity, a quantitative is a ramp over a range.
 *
 * It is the field that decides whether {@link Channel.domain} is a number at all, which is why it
 * is the one field of the entry that is not the writer's to spell.
 */
export type Scale = 'categorical' | 'quantitative';
