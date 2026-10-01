/**
 * A job, end to end in the browser: read the documents the program names, run the mapping on
 * DataFusion-WASM, write the `fossil/1` corpus, report the outcome.
 *
 * Every byte goes through `object_store` stores built from credentials the host vends — `read`
 * per connection the program names, `write` on the job. `DataFusion` reads a source through its
 * store by range requests, and the output is written through the job's, in parts when large.
 */
import { resolveDocuments } from '@fossil-lang/storage';
import {
  FossilError,
  HOST_MS,
  attachCause,
  isFossilError,
  within,
  type Host,
  type HostCall,
  type Problem,
  type Related,
  type UnreadDocument,
} from '@fossil-lang/types';

import { FossilExecutor } from './client.js';
import type { RunReport } from './index.js';

/** What {@link runJob} runs as: the job, the host that vends its credentials, and its outcome. */
export interface Job {
  /** The job the output is written for — `{ job: id }` is the scope `write` is vended on. */
  id: string;
  host: Host;
  /** Report the run outcome (PATCH the job). A host promise like any other: 30 s to answer. */
  complete(req: CompletePayload, options: HostCall): Promise<void>;
}

/** The completion payload the host PATCHes back. */
export interface CompletePayload {
  status: 'completed' | 'failed';
  /**
   * The run's report — where it wrote and what it dropped. The field keeps the
   * name its consumers read; the manifest itself is `<dest>fossil.json`.
   */
  manifest?: RunReport;
  /** Why it failed, as plain data — `FossilError.from(problem)` rebuilds the error. */
  problem?: Problem;
}

/**
 * How long a completion is retried: the server's lease (`/docs/design/failure`, *A job always reaches
 * an end*). Past it the server's sweep has ended the job as abandoned, and a report would be late.
 */
export const REPORT_MS = 60_000;
/** The first pause between two attempts to report; each pause after doubles it. */
const BACKOFF_MS = 1_000;

/**
 * Run a fossil mapping in the browser, end-to-end, and report its outcome once.
 *
 * On success it reports `completed` and returns the `RunReport`. On a failure — a document or source
 * that could not be read included — it reports `failed` with the problem and throws the
 * `FossilError`; a failure fossil did not raise — a panic — is `internal/bug`, the original kept as
 * its cause. A report the host refuses is tried again until {@link REPORT_MS} has passed.
 *
 * - A run that wrote and could not report it is **not** reported `failed`: it throws the reporting
 *   failure — `storage/host-refused` or `storage/host-silent` about the completion — and the job's
 *   output is there.
 * - A run that failed and could not report that either throws the run's failure, with the
 *   reporting failure at the end of its causes.
 * - `signal` stops the run and rejects with its reason. A stopped run is not reported: there is no
 *   code for it, and the server's lease sweep ends the job.
 *
 * `initFossilExecutor` must have resolved first.
 */
export async function runJob(
  program: string,
  job: Job,
  { signal }: { signal?: AbortSignal } = {},
): Promise<RunReport> {
  let report: RunReport;
  try {
    report = await execute(program, job, signal);
  } catch (e) {
    if (signal?.aborted && e === signal.reason) throw e;
    const failure = isFossilError(e)
      ? e
      : FossilError.of('internal/bug', { what: 'the run failed outside fossil' }, {
          cause: e,
        });
    try {
      await complete(job, { status: 'failed', problem: failure.problem });
    } catch (unreported) {
      attachCause(failure, unreported);
    }
    throw failure;
  }
  await complete(job, { status: 'completed', manifest: report });
  return report;
}

async function execute(program: string, job: Job, signal: AbortSignal | undefined): Promise<RunReport> {
  const exec = new FossilExecutor(program);
  try {
    const { unread } = await resolveDocuments(exec, job.host, { signal });
    if (unread.length > 0) throw unreadable(unread);
    return await exec.run(job.host, job.id, signal);
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

/** Report `payload`, each attempt within 30 s, until {@link REPORT_MS} has passed. */
async function complete(job: Job, payload: CompletePayload): Promise<void> {
  const scope = `the completion of job ${job.id}`;
  const deadline = Date.now() + REPORT_MS;
  for (let pause = BACKOFF_MS; ; pause *= 2) {
    try {
      await within(HOST_MS, (signal) => job.complete(payload, { signal }), {
        silent: (after) =>
          FossilError.of('storage/host-silent', { scope, after }),
      });
      return;
    } catch (cause) {
      if (Date.now() + pause >= deadline) {
        if (isFossilError(cause, 'storage/host-silent')) throw cause;
        throw FossilError.of('storage/host-refused', { scope }, { cause });
      }
      await new Promise((resolve) => setTimeout(resolve, pause));
    }
  }
}
