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
 * **Reporting the outcome is the host's.** A run answers or throws its `FossilError`; what the host
 * records about a job — and how long it keeps trying to — is its own lifecycle.
 */
import { FossilExecutor } from '../pkg/fossil_df_wasm.js';
import { resolveDocuments } from '@fossil-lang/storage';
import { FossilError, isFossilError, type Host, type Profiled, type RunReport } from '@fossil-lang/types';
import { until, type MissingDocument, type Related, type UnreadDocument } from '@fossil-lang/types/internal';

import { initFossilExecutor } from './load.js';

/** One file of the written corpus: a path relative to its root + its encoded bytes. */
export interface CorpusFile {
  /** `fossil.json`, `vertex/<Type>.parquet` or `edge/<Src>_<label>_<Dst>.parquet`. */
  path: string;
  bytes: Uint8Array;
}

/** What every run takes, whichever way it reads and writes. */
interface Common {
  /** At most this many rows read from each source scan, before any join. An edge whose endpoint
   *  fell outside the sample is dropped and counted in `report.dropped`. */
  limit?: number;
  /** What the editor's `FossilProgram.describe` answered, so the run checks the program against
   *  the columns the editor checked it against.
   *  Without it every source is an open row. */
  profiled?: Profiled;
  /** Stops the run; it rejects with the signal's reason. */
  signal?: AbortSignal;
}

/** A run that reads through a host. */
export interface HostRun extends Common {
  /** The host that vends the credentials of every read, and of the write into a job. */
  host: Host;
  /** `{ job }` — the job's storage, through the `write` credential vended on it; `'memory'` — an
   *  `InMemory`, answered as `{ report, files }`, with no `write` asked for. */
  into: { job: string } | 'memory';
}

/** A run over bytes in hand: there is no host to vend a write, so it writes into memory. */
export interface FilesRun extends Common {
  /** Every document and source the program reads, as bytes, by its location. */
  files: Readonly<Record<string, Uint8Array>>;
  /** Where the program lives — a URL such as `https://corpus.invalid/hello.fossil` — so the
   *  relative documents and sources it names resolve beside it. */
  path: string;
  into?: 'memory';
}

/** Where a run reads, where it writes, and how much. */
export type RunOptions = HostRun | FilesRun;

/** What a run into memory answers: the report and every file of the corpus. */
export interface InMemoryRun {
  report: RunReport;
  files: CorpusFile[];
}

/**
 * Run a fossil mapping and write its corpus: the report — where it wrote, what the edge joins
 * dropped — and, into memory, the corpus itself. Boots the executor module itself.
 *
 * Rejects with a `FossilError`: `api/invalid-argument` for `{ files, into: { job } }` or a `limit`
 * that is not a whole number; `document/unread` for a document that could not be read;
 * `run/over-budget` for a run that needed more than the executor's 2 GiB, decided while the graph
 * executes, so nothing has been written; `internal/bug` for a failure fossil did not raise — a
 * panic — the original kept as its cause. A stopped run rejects with the signal's reason, and
 * since `fossil.json` is written last it leaves no corpus.
 */
export function run(program: string, options: HostRun & { into: { job: string } }): Promise<RunReport>;
export function run(program: string, options: (HostRun & { into: 'memory' }) | FilesRun): Promise<InMemoryRun>;
export function run(program: string, options: RunOptions): Promise<RunReport | InMemoryRun>;
export async function run(program: string, options: RunOptions): Promise<RunReport | InMemoryRun> {
  const { profiled, signal, limit } = options;
  try {
    // With a host `into` has no default: a host that wrote `{ host, job }` is refused by the
    // executor as `api/invalid-argument`, not run into memory.
    const into = 'files' in options ? (options.into ?? 'memory') : options.into;
    if ('files' in options && into !== 'memory') {
      throw FossilError.of('api/invalid-argument', {
        argument: 'into',
        expected: "'memory' for a run over files, which has no host to vend a write",
      });
    }
    await until(initFossilExecutor(), signal);
    const exec = new FossilExecutor(program, 'files' in options ? options.path : undefined);
    try {
      if (profiled !== undefined) describedAs(exec, profiled);
      if ('host' in options) {
        const { unread } = await resolveDocuments(exec, options.host, { signal });
        if (unread.length > 0) throw unreadable(unread);
      } else {
        const unread = registerFrom(exec, options.files);
        if (unread.length > 0) throw unreadable(unread);
      }
      const reads = 'host' in options ? { host: options.host } : { files: options.files };
      return (await exec.run(reads, into, limit, signal)) as RunReport | InMemoryRun;
    } finally {
      exec.free();
    }
  } catch (e) {
    if (signal?.aborted && e === signal.reason) throw e;
    if (isFossilError(e)) throw e;
    throw FossilError.of('internal/bug', { what: 'the run failed outside fossil' }, { cause: e });
  }
}

/** What `FossilProgram.describe` gave the editor's checker, into the run's database. */
function describedAs(exec: FossilExecutor, { descriptors, undescribed }: Profiled): void {
  for (const descriptor of descriptors) exec.registerInferredDescriptor(JSON.stringify(descriptor));
  for (const { source, problem } of undescribed) exec.registerUndescribed(source.key, JSON.stringify(problem));
}

/** `resolveDocuments` over bytes in hand: every document the program names, read from `files`
 *  until nothing new is missing, and the ones `files` does not hold. */
function registerFrom(exec: FossilExecutor, files: Readonly<Record<string, Uint8Array>>): UnreadDocument[] {
  const decoder = new TextDecoder();
  const unread: UnreadDocument[] = [];
  for (;;) {
    const missing = (exec.missingDocuments() as MissingDocument[]).filter(
      (d) => !unread.some((u) => u.key === d.key),
    );
    if (missing.length === 0) return unread;
    for (const d of missing) {
      const bytes = files[d.location];
      if (bytes === undefined) {
        unread.push({ ...d, problem: FossilError.of('source/not-found', { location: d.location }).problem });
      } else {
        exec.registerDocument(d.key, decoder.decode(bytes));
      }
    }
  }
}

/**
 * `document/unread`, every document's problem kept. One is the cause. Several are an
 * `AggregateError` of their `FossilError`s, and each is a `related` entry — its code, data and
 * detail — on the wire, where a problem carries one cause.
 */
function unreadable(unread: readonly UnreadDocument[]): FossilError<'document/unread'> {
  const documents = unread.map((d) => d.key);
  if (unread.length === 1) return FossilError.of('document/unread', { documents }, { cause: unread[0]!.problem });
  return FossilError.of('document/unread', { documents }, {
    cause: new AggregateError(
      unread.map((d) => FossilError.from(d.problem)),
      `${unread.length} documents could not be read`,
    ),
    related: unread.map(
      ({ problem }) =>
        ({
          code: problem.code,
          data: problem.data,
          title: problem.title,
          detail: problem.detail,
          severity: problem.severity,
          ...(problem.help === undefined ? {} : { help: problem.help }),
        }) as Related,
    ),
  });
}
