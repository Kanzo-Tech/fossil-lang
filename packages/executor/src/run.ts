/**
 * A run, end to end in the browser: read the documents the program names, run the mapping on
 * DataFusion-WASM, write the `fossil/1` corpus under the job.
 *
 * Every byte goes through `object_store` stores built from credentials the host vends — `read`
 * per connection the program names, `write` on the job. `DataFusion` reads a source through its
 * store by range requests, and the output is written through the job's, in parts when large.
 *
 * **Reporting the outcome is the host's.** A run answers its report or throws its `FossilError`;
 * what the host records about a job — and how long it keeps trying to — is its own lifecycle, and
 * keasy's runner is where it lives.
 */
import { resolveDocuments } from '@fossil-lang/storage';
import { FossilError, isFossilError, type Host, type Related, type UnreadDocument } from '@fossil-lang/types';

import { FossilExecutor } from './client.js';
import type { RunReport } from './index.js';

/** Where a run reads and writes: the host that vends its credentials, and the job it writes for. */
export interface RunOptions {
  host: Host;
  /** The job the output is written for — `{ job }` is the scope `write` is vended on. */
  job: string;
  /** Stops the run; it rejects with the signal's reason. */
  signal?: AbortSignal;
}

/**
 * Run a fossil mapping in the browser and write its corpus under the job: the report — where it
 * wrote, what the edge joins dropped — or the `FossilError` that stopped it. A document or source
 * that could not be read is `document/unread`; a failure fossil did not raise — a panic — is
 * `internal/bug`, the original kept as its cause; a stopped run rejects with the signal's reason.
 *
 * `initFossilExecutor` must have resolved first.
 */
export async function run(program: string, { host, job, signal }: RunOptions): Promise<RunReport> {
  try {
    return await execute(program, host, job, signal);
  } catch (e) {
    if (signal?.aborted && e === signal.reason) throw e;
    if (isFossilError(e)) throw e;
    throw FossilError.of('internal/bug', { what: 'the run failed outside fossil' }, { cause: e });
  }
}

async function execute(program: string, host: Host, job: string, signal: AbortSignal | undefined): Promise<RunReport> {
  const exec = new FossilExecutor(program);
  try {
    const { unread } = await resolveDocuments(exec, host, { signal });
    if (unread.length > 0) throw unreadable(unread);
    return await exec.run(host, job, signal);
  } finally {
    exec.free();
  }
}

/**
 * `document/unread`, every document's problem kept. One is the cause. Several are an
 * `AggregateError` of their `FossilError`s, and each is a `related` entry — its code, data and
 * detail — on the wire, where a problem carries one cause.
 */
function unreadable(unread: readonly UnreadDocument[]): FossilError<'document/unread'> {
  const documents = unread.map((d) => d.key);
  const detail = `${documents.length} document(s) could not be read: ${documents.join(', ')}`;
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
