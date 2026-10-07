/**
 * A run, end to end: read the documents the program names, run the mapping on DataFusion-WASM,
 * write the `fossil/1` corpus. One door, and the destination is data — `object_store`'s `InMemory`
 * beside a cloud store behind one trait:
 *
 * - `{ host, job }` reads every byte through `object_store` stores built from the credentials the
 *   host vends — `read` per connection the program names, `write` on the job — and writes under
 *   the job's prefix. `DataFusion` reads a source by range requests, not whole.
 * - `{ files, path }` reads every document and source from `files`, by the location fossil
 *   resolves it to, and answers the corpus as bytes: the run a host with no storage — Node, a
 *   build script — makes.
 *
 * **Reporting the outcome is the host's.** A run answers or throws its `FossilError`; what the host
 * records about a job — and how long it keeps trying to — is its own lifecycle.
 */
import { FossilExecutor } from '../pkg/fossil_df_wasm.js';
import { resolveDocuments } from '@fossil-lang/storage';
import { FossilError, isFossilError, type Host, type RunReport } from '@fossil-lang/types';
import { until, type MissingDocument, type Related, type UnreadDocument } from '@fossil-lang/types/internal';

import { initFossilExecutor } from './load.js';

/** One file of the written corpus: a path relative to its root + its encoded bytes. */
export interface CorpusFile {
  /** `fossil.json`, `vertex/<Type>.parquet` or `edge/<Src>_<label>_<Dst>.parquet`. */
  path: string;
  bytes: Uint8Array;
}

/** Where a run reads and writes. */
export type RunOptions = (
  | {
      /** The host that vends the credentials of every read and of the write. */
      host: Host;
      /** The job the output is written for — `{ job }` is the scope `write` is vended on. */
      job: string;
    }
  | {
      /** Every document and source the program reads, as bytes, by its location. */
      files: Readonly<Record<string, Uint8Array>>;
      /** Where the program lives — a URL such as `https://corpus.invalid/hello.fossil` — so the
       *  relative documents and sources it names resolve beside it. */
      path: string;
    }
) & {
  /** Stops the run; it rejects with the signal's reason. */
  signal?: AbortSignal;
};

/** Where an in-memory run writes; `RunReport.location` names it. */
const MEMORY = 'memory://corpus/';

/**
 * Run a fossil mapping and write its corpus: the report — where it wrote, what the edge joins
 * dropped — and, run over `files`, the corpus itself. Boots the executor module itself.
 *
 * Rejects with a `FossilError`: `document/unread` for a document that could not be read;
 * `run/over-budget` for a run that needed more than the executor's 2 GiB, decided while the graph
 * executes, so nothing has been written; `internal/bug` for a failure fossil did not raise — a
 * panic — the original kept as its cause. A stopped run rejects with the signal's reason, and
 * since `fossil.json` is written last it leaves no corpus.
 */
export function run(program: string, options: RunOptions & { host: Host }): Promise<RunReport>;
export function run(
  program: string,
  options: RunOptions & { files: Readonly<Record<string, Uint8Array>> },
): Promise<{ report: RunReport; files: CorpusFile[] }>;
export async function run(
  program: string,
  options: RunOptions,
): Promise<RunReport | { report: RunReport; files: CorpusFile[] }> {
  const { signal } = options;
  try {
    await until(initFossilExecutor(), signal);
    const exec = new FossilExecutor(program, 'files' in options ? options.path : undefined);
    try {
      if ('host' in options) {
        const { unread } = await resolveDocuments(exec, options.host, { signal });
        if (unread.length > 0) throw unreadable(unread);
        return (await exec.run(options.host, options.job, signal)) as RunReport;
      }
      const unread = registerFrom(exec, options.files);
      if (unread.length > 0) throw unreadable(unread);
      return (await until(exec.runInMemory(options.files, MEMORY), signal)) as {
        report: RunReport;
        files: CorpusFile[];
      };
    } finally {
      exec.free();
    }
  } catch (e) {
    if (signal?.aborted && e === signal.reason) throw e;
    if (isFossilError(e)) throw e;
    throw FossilError.of('internal/bug', { what: 'the run failed outside fossil' }, { cause: e });
  }
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
