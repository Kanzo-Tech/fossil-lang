/**
 * The map from a corpus coordinate to the coordinate system the camera's rectangle is expressed in.
 *
 * ## Why this file exists at all
 *
 * `useQueryLoop` reads the camera with `screenToSpacePosition` and hands the resulting rectangle to
 * the source. That method answers in the coordinate system of the positions **cosmos.gl currently
 * holds**, and the source answers in the coordinate system **the corpus wrote**. The two are the
 * same only if nothing is rescaling in between.
 *
 * Something is. `setPointPositions(positions)` is called with no second argument, and cosmos.gl's
 * own documentation for that argument reads *«`true`: Don't rescale. `false` or `undefined`
 * (default): Use the behavior defined by `config.rescalePositions`»* — and with the simulation off,
 * which is this canvas's correct default, `rescalePositions` resolves to rescaling. So the map below
 * is rebuilt **from the extent of whichever answer just arrived**, and the rectangle the source is
 * asked about is a function of the previous answer rather than of the camera.
 *
 * ## Measured, not modelled
 *
 * This is cosmos.gl 3.4.1's `rescaleInitialNodePositions`, and it is transcribed rather than
 * invented — but a transcription is a claim, so it was checked against the library through its own
 * published getter, `Graph.getScaleX()`, in a browser with a real WebGL device, over the bench
 * corpus's extent and three sample shapes. Predicted against measured:
 *
 * | sample                        | `c` predicted | `c` from `getScaleX()` |
 * | ----------------------------- | ------------- | ---------------------- |
 * | 15,625 points over the extent | 1.2000        | **1.2003**             |
 * | 8,000 points over a 30% box   | 0.3333        | **0.3334**             |
 * | 2,500 points over a 10% box   | 1.0000        | **1.0004**             |
 *
 * With the camera untouched between the three, one corpus coordinate — (500, 500) — read back at
 * space 1547.07, 1590.89 and 1556.91. The same fixed camera rectangle therefore asks the source
 * about x[175.7, 925.5], x[−798.8, 1900.5] and x[101.1, 1000.7]: **12.9× the area between the
 * first and the second**. `frame-probe.html` is that measurement, and it is the reproduction.
 */

/** The identity frame — what a renderer that is told not to rescale applies. */
export const PINNED = { c: 1, vx: 0, vy: 0, minx: 0, miny: 0 };

/**
 * The frame cosmos.gl derives from one uploaded sample.
 *
 * `positions` is the whole buffer it is handed — marks AND anchors, because that is what
 * `setPointPositions` receives — so a far end dragged in by an edge widens the extent this is
 * computed from, which is one of the ways the map moves without the camera moving.
 */
export function rescaledFrame(positions, spaceSize) {
  const n = positions.length / 2;
  if (n === 0) return PINNED;
  let minx = Infinity;
  let maxx = -Infinity;
  let miny = Infinity;
  let maxy = -Infinity;
  for (let i = 0; i < n; i += 1) {
    const x = positions[i * 2];
    const y = positions[i * 2 + 1];
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
    if (x < minx) minx = x;
    if (x > maxx) maxx = x;
    if (y < miny) miny = y;
    if (y > maxy) maxy = y;
  }
  if (!Number.isFinite(minx)) return PINNED;
  const dx = maxx - minx;
  const dy = maxy - miny;
  const u = Math.max(dx, dy);
  // Two branches where the library gives up and leaves the positions alone: a degenerate sample,
  // and one wider than the declared space.
  if (u === 0 || u > spaceSize) return PINNED;
  // `spaceSize² · 1e-3` is the threshold between the two fill fractions, and the jump across it is
  // 12× — which is why a picture can break on a pan that changes nothing but how many points came
  // back.
  const dense = spaceSize * spaceSize * 1e-3;
  const fill = n > dense ? spaceSize * Math.max(1.2, Math.sqrt(n) / spaceSize) : spaceSize * 0.1;
  const c = fill / u;
  const pad = (spaceSize - fill) / 2;
  return {
    c,
    vx: ((u - dx) / 2) * c + pad,
    vy: ((u - dy) / 2) * c + pad,
    minx,
    miny,
  };
}

/** A rectangle in the camera's coordinates, back in the corpus's own. */
export function toCorpus(frame, rect) {
  const inv = (v, v0, min) => (v - v0) / frame.c + min;
  return {
    xlo: inv(rect.xlo, frame.vx, frame.minx),
    xhi: inv(rect.xhi, frame.vx, frame.minx),
    ylo: inv(rect.ylo, frame.vy, frame.miny),
    yhi: inv(rect.yhi, frame.vy, frame.miny),
  };
}
