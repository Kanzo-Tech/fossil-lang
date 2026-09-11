/**
 * The crossfilter's mask over whatever is drawn — split out of `Canvas.tsx` unchanged.
 */
import { useGraphContext } from '@kanzo-tech/graph';
import { useEffect, useRef } from 'react';

import { paintMask } from './crossfilter.js';

/**
 * How much alpha an excluded vertex keeps. Low enough to read as context, high enough that the
 * shape of what was filtered out is still visible — a crossfilter whose excluded rows vanish is a
 * filter, and the thing worth seeing is *where in the picture* the brushed range lives.
 */
const DIM = 0.07;

/**
 * The crossfilter's other end: the mask, painted onto whatever is drawn right now.
 *
 * A child of `GraphCanvas` for the same reason `PinFrame` is one — `useGraphContext` is the seam
 * `@kanzo-tech/graph` publishes, and the graph instance is built in an effect inside that element.
 *
 * **Two things move independently and this has to survive both.** The mask changes when the reader
 * brushes; the resident set changes when the camera moves, and a camera move re-uploads colours
 * from the source, discarding the fade. So the effect depends on both, and it re-captures the
 * baseline whenever the slice identity changes: the undimmed upload is what a fade is computed
 * from, and reading back an already-faded buffer to fade it again compounds — four brush moves
 * would take a live vertex to invisible.
 *
 * The retry loop is not defensive padding. `slice` becomes the new answer before cosmos.gl has been
 * handed the buffers for it, so for a frame or two `getPointColors()` is the previous answer's
 * array while `resident` is the new one's map. Painting then would fade whichever vertices happen
 * to sit at those indices now, which is exactly the buffer-index-is-not-an-identity failure
 * `resident.ts` exists to prevent. `paintMask` reports the mismatch and this waits a macrotask —
 * not a frame: `requestAnimationFrame` does not fire in a tab that is not visible, and the panel's
 * own verifier drives it in one that is not.
 */
export default function Mask({ mask }: { mask: Uint8Array | null }) {
  const { getGraph, getResident, slice } = useGraphContext();
  const baseline = useRef<{ for: unknown; colors: Float32Array } | null>(null);

  useEffect(() => {
    let live = true;
    const paint = () => {
      if (!live) return;
      const graph = getGraph();
      if (graph === null) {
        setTimeout(paint, 16);
        return;
      }
      // A new answer invalidates the baseline: different vertices, different order, different
      // length. Captured before anything here has touched the buffer, so it is kanzo's colouring.
      if (baseline.current === null || baseline.current.for !== slice) {
        const colors = graph.getPointColors();
        if (colors.length === 0) {
          setTimeout(paint, 16);
          return;
        }
        baseline.current = { for: slice, colors: new Float32Array(colors) };
      }
      if (!paintMask(graph, getResident(), baseline.current.colors, mask, DIM)) {
        // The renderer has not caught up with this answer yet. Drop the stale baseline so the
        // retry captures the right one rather than fading the previous frame's colours.
        baseline.current = null;
        setTimeout(paint, 16);
      }
    };
    paint();
    return () => {
      live = false;
    };
  }, [getGraph, getResident, mask, slice]);

  return null;
}
