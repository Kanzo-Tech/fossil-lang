/**
 * A run, end to end: read the documents the program names, run the mapping on DataFusion-WASM,
 * write the `fossil/1` corpus. One door, with two independent axes and a limit
 * (`/docs/design/three-hosts`):
 *
 * - **Reads.** `host` reads every byte through `object_store` stores built from the credentials
 *   the host vends — `read` per connection the program names — and `DataFusion` reads a source by
 *   range requests, not whole. `files` + `path` reads every document and source from bytes in
 *   hand, by the location fossil resolves it to: the run a host with no storage — Node, a build
 *   script — makes.
 * - **Writes.** `into: { job }` writes under the prefix the host vends `write` on for the job, and
 *   answers the `RunReport`. `into: 'memory'` writes into an `object_store` `InMemory`, asks for
 *   no `write`, and answers `{ report, files }`. With `files`, `into` is `'memory'`.
 * - **`limit`** reads at most that many rows from each source scan, before any join; an edge whose
 *   endpoint fell outside the sample is dropped and counted in `report.dropped`.
 *
 * **Where it runs is the platform's** (`/docs/design/off-the-page`): in a module worker the
 * executor starts where `globalThis.Worker` is a constructor — a browser, Deno, Bun — so the page
 * keeps painting and its Stop can be pressed; in this thread in Node. One worker per run, ended
 * with it: a wasm instance's memory never shrinks.
 *
 * **Reporting the outcome is the host's.** A run answers or throws its `FossilError`; what the host
 * records about a job — and how long it keeps trying to — is its own lifecycle.
 */
import { type RunReport } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { failingAsFossil, intoOf, runHere, type FilesRun, type HostRun, type InMemoryRun, type RunOptions } from './here.js';
import { compiledExecutor, hasWorker } from './load.js';
import { drive, type Request } from './protocol.js';

export type { FilesRun, HostRun, InMemoryRun, RunOptions } from './here.js';

/**
 * Run a fossil mapping and write its corpus: the report — where it wrote, what the edge joins
 * dropped — and, into memory, the corpus itself. Boots the executor module itself.
 *
 * Rejects with a `FossilError`: `api/invalid-argument` for `{ files, into: { job } }` or a `limit`
 * that is not a whole number; `document/unread` for a document that could not be read;
 * `run/over-budget` for a run that needed more than the executor's 2 GiB, decided while the graph
 * executes, so nothing has been written; `module/unreachable` for an executor — its module, or its
 * worker — that could not be loaded; `internal/bug` for a failure fossil did not raise — a panic, a
 * worker that died — the original kept as its cause. A stopped run rejects with the signal's reason,
 * and since `fossil.json` is written last it leaves no corpus; in a worker, the worker is ended at
 * once.
 */
export function run(program: string, options: HostRun & { into: { job: string } }): Promise<RunReport>;
export function run(program: string, options: (HostRun & { into: 'memory' }) | FilesRun): Promise<InMemoryRun>;
export function run(program: string, options: RunOptions): Promise<RunReport | InMemoryRun>;
export async function run(program: string, options: RunOptions): Promise<RunReport | InMemoryRun> {
  if (!hasWorker()) return runHere(program, options);
  const { signal, limit, profiled } = options;
  return failingAsFossil(signal, async () => {
    const into = intoOf(options);
    const module = await until(compiledExecutor(), signal);
    const request: Request = {
      type: 'run',
      module,
      program,
      into,
      limit,
      profiled,
      host: 'host' in options,
      ...('files' in options ? { files: options.files, path: options.path } : {}),
    };
    const worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
    return (await drive(worker, request, {
      host: 'host' in options ? options.host : undefined,
      signal,
      terminate: () => worker.terminate(),
      crashes: (listener) =>
        worker.addEventListener('error', (event) => {
          event.preventDefault();
          listener(event.message || 'the worker raised an error');
        }),
    })) as RunReport | InMemoryRun;
  });
}


