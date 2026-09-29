/**
 * Half two of the loop: run the program, get a corpus.
 *
 * `@fossil-lang/executor` is DataFusion compiled to wasm. It is the heavy half — an order
 * of magnitude larger than the checker — so it is dynamically imported and only when the
 * user presses Run. A playground that pays for the executor to show a red squiggle has
 * spent its whole load budget on the wrong half.
 *
 * The executor reads every source through an `object_store` store. In keasy's shape the stores
 * are built from credentials the host vends; here they are in memory, filled from a build-time
 * import, and the executor cannot tell — which is what makes «the source data never leaves the
 * machine» a property of the architecture rather than a promise in a README.
 */
import type { ExecutorResult } from '@fossil-lang/executor';

import { measured, type BundleCost } from './check.js';
import { absolutise, DEST, registerBundled, SOURCE_BYTES } from './example.js';

let cost: BundleCost | null = null;

/** The measured cost of the executor bundle, or `null` before the first {@link run}. */
export function executorCost(): BundleCost | null {
  return cost;
}

/**
 * Compile and execute `program`, returning the GraphAr files and the manifest.
 *
 * `initFossilExecutor` is memoised, so the cost is only paid once and the measurement only
 * recorded once; see {@link measured} for where the size comes from.
 */
export async function run(program: string): Promise<ExecutorResult> {
  const { FossilExecutor, initFossilExecutor } = await import('@fossil-lang/executor');

  if (!cost) {
    const started = performance.now();
    await initFossilExecutor();
    cost = measured('fossil_df_wasm_bg', started);
  }

  // The one rewrite, and `example.ts` argues it: the executor's object store is keyed by a
  // URI's scheme+authority, and a relative path has neither. The checker never sees this.
  const executor = new FossilExecutor(absolutise(program));
  try {
    // The shape document arrives the way the checker's does, from the bundle: the run
    // decodes its output contract from it and refuses without it.
    registerBundled(executor);
    const missing = executor.missingDocuments();
    if (missing.length > 0) {
      throw new Error(`the program names ${missing.map((d) => d.key).join(', ')}, which this playground has no text for.`);
    }

    // `sources()` is pure — it reads the program and says what it reads, without reading.
    const unheld = executor.sources().filter((source) => !SOURCE_BYTES[source.uri]);
    if (unheld.length > 0) {
      throw new Error(
        `the program reads ${unheld.map((s) => s.uri).join(', ')}, which this playground has no bytes for. ` +
          `It ships the walking skeleton's sources only — see src/example.ts.`,
      );
    }

    return await executor.runInMemory(SOURCE_BYTES, DEST);
  } finally {
    executor.free();
  }
}
