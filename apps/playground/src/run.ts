/**
 * Half two of the loop: run the program, get a corpus.
 *
 * `@fossil-lang/executor` is DataFusion compiled to wasm. It is the heavy half — an order
 * of magnitude larger than the checker — so it is dynamically imported and only when the
 * user presses Run. A playground that pays for the executor to show a red squiggle has
 * spent its whole load budget on the wrong half.
 *
 * The host's job in `run` is spelled out by the crate: enumerate the sources, stage each
 * one's bytes, hand them over. Natively the bytes come from disk; in keasy's shape they
 * come from a signed URL. Here they come from a build-time import, and the executor cannot
 * tell — which is what makes «the source data never leaves the machine» a property of the
 * architecture rather than a promise in a README.
 */
import type { ExecutorResult, SourceInput } from '@fossil-lang/executor';

import type { BundleCost } from './check.js';
import { absolutise, DEST, SHEX, SOURCE_BYTES } from './example.js';

let cost: BundleCost | null = null;

/** The measured cost of the executor bundle, or `null` before the first {@link run}. */
export function executorCost(): BundleCost | null {
  return cost;
}

/**
 * Compile and execute `program`, returning the GraphAr files and the manifest.
 *
 * The `.wasm` is fetched as bytes so its size is measurable; `initFossilExecutor` is
 * memoised, so the cost is only paid once and the measurement only recorded once.
 */
export async function run(program: string): Promise<ExecutorResult> {
  const [{ FossilExecutor, initFossilExecutor }, { default: wasmUrl }] = await Promise.all([
    import('@fossil-lang/executor'),
    import('@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm?url'),
  ]);

  if (!cost) {
    const started = performance.now();
    const buffer = await (await fetch(wasmUrl)).arrayBuffer();
    await initFossilExecutor({
      wasmUrl: new Response(buffer, { headers: { 'content-type': 'application/wasm' } }),
    });
    cost = { bytes: buffer.byteLength, ms: Math.round(performance.now() - started) };
  }

  const executor = new FossilExecutor();

  // The one rewrite, and `example.ts` argues it: the executor's object store is keyed by a
  // URI's scheme+authority, and a relative path has neither. The checker never sees this.
  program = absolutise(program);

  // `sources()` is pure — it reads the program and says what to fetch, without fetching.
  // The shape document is passed alongside because a program's sources include the ShEx it
  // names, and the executor parses it to know the output contract.
  const wanted = executor.sources(program, {}, SHEX);

  const staged: SourceInput[] = wanted.map((source) => {
    const bytes = SOURCE_BYTES[source.uri];
    if (!bytes) {
      throw new Error(
        `the program reads ${source.uri}, which this playground has no bytes for. ` +
          `It ships the walking skeleton's sources only — see src/example.ts.`,
      );
    }
    return { ...source, bytes };
  });

  return executor.run(program, staged, DEST, {}, SHEX);
}
