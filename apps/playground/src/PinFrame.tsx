/**
 * Pinning the renderer's frame — split out of `Canvas.tsx` unchanged.
 *
 * A file of its own because it is not part of drawing a corpus: it is a workaround for one
 * cosmos.gl default, it carries the measurement that justifies it, and the comment says the
 * fix belongs upstream. When `GraphCanvas` grows the guarantee, this file is what is deleted.
 */
import { useGraphContext } from '@kanzo-tech/graph';
import { useEffect } from 'react';

/**
 * **Pin the frame**, which is the one line between a camera that refines and one that breaks.
 *
 * cosmos.gl rescales the positions it is handed to fill its space, deriving the map from the extent
 * of *that upload*. With the simulation off — the correct default here — `rescalePositions` resolves
 * to on, and `setPointPositions` is called with no second argument. So every answer rebuilds the map
 * from itself, and `screenToSpacePosition`, which the query loop reads the camera with, starts
 * speaking a coordinate system the corpus never wrote. The rectangle the source is asked about is
 * then a function of the PREVIOUS answer.
 *
 * Measured over the bench corpus, two routes to the same camera rectangle: straight in, the source
 * is asked about x[−981, 347] and draws 8,395; wandering in and out and back, it is asked about
 * x[−3719, −1022], which is off the corpus, and draws **nothing**. Pinned, both ask x[−253, 1355]
 * and draw the same 18,308. `scripts/verify-properties.mjs` is that measurement and
 * `/frame-probe.html` is where the arithmetic was checked against the library's own `getScaleX()`.
 *
 * A component rather than a prop because `@kanzo-tech/graph` publishes no passthrough for cosmos.gl
 * config — `useGraphContext` is the seam it does publish, and a child of `GraphCanvas` is where it
 * can be read. **The fix belongs upstream**: a renderer whose source hands back the coordinates its
 * next query is expressed in must not move them, and the day `GraphCanvas` says so this goes.
 */
export default function PinFrame() {
  const { getGraph } = useGraphContext();
  useEffect(() => {
    let live = true;
    // The renderer is built in an effect of its own and there is no ready signal to await from out
    // here, so this waits for the instance rather than assuming it. A macrotask and not a frame:
    // `requestAnimationFrame` does not fire in a tab that is not visible, and this must not depend
    // on being watched.
    const pin = () => {
      if (!live) return;
      const graph = getGraph();
      if (graph === null) {
        setTimeout(pin, 16);
        return;
      }
      graph.setConfigPartial({ rescalePositions: false });
    };
    pin();
    return () => {
      live = false;
    };
  }, [getGraph]);
  return null;
}
