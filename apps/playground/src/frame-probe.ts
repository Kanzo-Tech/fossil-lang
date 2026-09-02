/**
 * The reproduction, minimal: does the renderer move the corpus under the camera on every answer?
 *
 * **Not part of the app.** It is a second entry point Vite serves in dev and does not bundle, and
 * it exists because the defect it measures is in a WebGL device: `scripts/verify-properties.mjs`
 * asserts the property in Node against a transcription of the arithmetic, and this is the file that
 * checked the transcription against the library. Open `/frame-probe.html` with the dev server up.
 *
 * `useQueryLoop` reads the camera with `screenToSpacePosition`, which answers in the coordinate
 * system of the positions cosmos.gl currently HOLDS, and hands that rectangle to the source, which
 * answers in the coordinate system the corpus WROTE. The two agree only if cosmos.gl is not
 * rescaling. `setPointPositions(p)` is called with no second argument, and its own doc says the
 * second argument is "don't rescale"; with the simulation off, `config.rescalePositions` defaults
 * to rescaling. So the map from corpus coordinate to space coordinate is rebuilt from the extent
 * of whichever sample just arrived.
 *
 * Measured here as `getScaleX()`, cosmos.gl's own published getter for that map.
 */
import { Graph } from '@cosmos.gl/graph';

const out = document.getElementById('out')!;
const host = document.getElementById('c') as HTMLDivElement;

/** The real bench corpus's extent, off its Parquet footer. */
const X0 = -1056.9276;
const X1 = 2158.7346;
const Y0 = -1058.9626;
const Y1 = 2059.5588;
const SPACE = Math.max(X1 - X0, Y1 - Y0);

const PX = 500;
const PY = 500;

function sample(n: number, f: number): Float32Array {
  const a = new Float32Array(n * 2);
  a[0] = PX;
  a[1] = PY;
  const cx = (X0 + X1) / 2;
  const cy = (Y0 + Y1) / 2;
  const w = (X1 - X0) * f;
  const h = (Y1 - Y0) * f;
  let s = 12345;
  const rnd = () => ((s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
  for (let i = 1; i < n; i++) {
    a[i * 2] = cx - w / 2 + rnd() * w;
    a[i * 2 + 1] = cy - h / 2 + rnd() * h;
  }
  return a;
}

const graph = new Graph(host, {
  enableSimulation: false,
  spaceSize: SPACE,
  fitViewOnInit: false,
  pixelRatio: 1,
  attribution: '',
});
(globalThis as unknown as { G: unknown }).G = graph;

const log: string[] = [];

/** The rectangle `useQueryLoop.viewOf` would hand the source, verbatim. */
function viewOf() {
  const { height, width } = host.getBoundingClientRect();
  const [ax, ay] = graph.screenToSpacePosition([0, 0]);
  const [bx, by] = graph.screenToSpacePosition([width, height]);
  return {
    xMin: Math.min(ax!, bx!),
    yMin: Math.min(ay!, by!),
    xMax: Math.max(ax!, bx!),
    yMax: Math.max(ay!, by!),
  };
}

async function step(label: string, n: number, f: number) {
  graph.setPointPositions(sample(n, f));
  graph.render();
  await new Promise((r) => setTimeout(r, 200));
  const sx = graph.getScaleX();
  const v = viewOf();
  log.push(
    `${label.padEnd(30)} n=${String(n).padStart(6)}  scaleX(${PX})=${sx ? sx(PX).toFixed(2) : 'identity'}` +
      `  camera rect x[${v.xMin.toFixed(1)}, ${v.xMax.toFixed(1)}]`,
  );
  out.textContent = log.join('\n');
}

await graph.ready;
graph.setZoomLevel(1, 0);
await new Promise((r) => setTimeout(r, 300));

await step('A: far view', 15625, 1.0);
await step('B: 30% window', 8000, 0.3);
await step('C: far view again', 15625, 1.0);
await step('D: 10% window', 2500, 0.1);
await step('E: far view again', 15625, 1.0);

log.push('');
log.push('The camera was set once and never touched. If scaleX(500) differs between rows, the map');
log.push('from corpus coordinate to the space the camera rectangle is expressed in is a function of');
log.push('WHICH SAMPLE ARRIVED — so the same rectangle reached by two routes is two pictures.');
out.textContent = log.join('\n');
