/**
 * A run in this thread — what {@link runHere} is: the whole of a run where the platform has no
 * worker (Node), and what the executor's worker runs where it has one (`./worker.ts`). The door and
 * its options are `./run.ts`'s.
 */
import { FossilExecutor } from '../pkg/fossil_df_wasm.js';
import { resolveDocuments } from '@fossil-lang/storage';
import { FossilError, isFossilError, type CorpusFile, type Host, type Profiled, type RunReport } from '@fossil-lang/types';
import { until, type MissingDocument, type Related, type UnreadDocument } from '@fossil-lang/types/internal';

import { bootHere } from './load.js';

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

/** What a run into memory answers: the report and every file of the graph. */
export interface InMemoryRun {
  report: RunReport;
  files: CorpusFile[];
}

/**
 * {@link run}, in this thread: what a run is where there is no worker — Node — and what the
 * executor's worker runs where there is one. Boots the executor module itself.
 *
 * Rejects with a `FossilError`: `api/invalid-argument` for `{ files, into: { job } }` or a `limit`
 * that is not a whole number; `document/unread` for a document that could not be read;
 * `run/over-budget` for a run that needed more than the executor's 2 GiB, decided while the graph
 * executes, so nothing has been written; `internal/bug` for a failure fossil did not raise — a
 * panic — the original kept as its cause. A stopped run rejects with the signal's reason, and
 * since `fossil.graph.yml` is written last it leaves no graph.
 */
export async function runHere(program: string, options: RunOptions): Promise<RunReport | InMemoryRun> {
  const { profiled, signal, limit } = options;
  return failingAsFossil(signal, async () => {
    const into = intoOf(options);
    await until(bootHere(), signal);
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
  });
}

/**
 * Where a run writes. With a host `into` has no default: a host that wrote `{ host, job }` is
 * refused by the executor as `api/invalid-argument`, not run into memory.
 */
export function intoOf(options: RunOptions): { job: string } | 'memory' {
  const into = 'files' in options ? (options.into ?? 'memory') : options.into;
  if ('files' in options && into !== 'memory') {
    throw FossilError.of('api/invalid-argument', {
      argument: 'into',
      expected: "'memory' for a run over files, which has no host to vend a write",
    });
  }
  return into;
}

/**
 * `body`, whose failure is a `FossilError` or the signal's reason: anything else — a panic — is
 * `internal/bug`, the original kept as its cause.
 */
export async function failingAsFossil<T>(signal: AbortSignal | undefined, body: () => Promise<T>): Promise<T> {
  try {
    return await body();
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
