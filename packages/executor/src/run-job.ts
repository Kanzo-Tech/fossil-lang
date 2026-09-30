/**
 * A job, end to end in the browser: read the documents the program names, run the mapping on
 * DataFusion-WASM, write the `fossil/1` corpus, report the outcome.
 *
 * Every byte goes through `object_store` stores built from credentials the host vends — `read`
 * per connection the program names, `write` on the job. `DataFusion` reads a source through its
 * store by range requests, and the output is written through the job's, in parts when large.
 */
import { resolveDocuments } from '@fossil-lang/storage';
import { FossilError, isFossilError, type Host, type Problem } from '@fossil-lang/types';

import { FossilExecutor } from './client.js';
import type { RunReport } from './index.js';

/** What {@link runJob} runs as: the job, the host that vends its credentials, and its outcome. */
export interface Job {
  /** The job the output is written for — `{ job: id }` is the scope `write` is vended on. */
  id: string;
  host: Host;
  /** Report the run outcome (PATCH the job). */
  complete(req: CompletePayload): Promise<void>;
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
 * Run a fossil mapping in the browser, end-to-end. Returns the `RunReport` on success; on any
 * failure — a document or source that could not be read included — reports a `failed` completion
 * with its problem (best-effort) and throws the `FossilError`. A failure fossil did not raise — a
 * panic — is `internal/bug`, the original kept as its cause.
 *
 * `initFossilExecutor` must have resolved first.
 */
export async function runJob(program: string, job: Job): Promise<RunReport> {
  let exec: FossilExecutor | undefined;
  try {
    exec = new FossilExecutor(program);

    const { unread } = await resolveDocuments(exec, job.host);
    if (unread.length > 0) {
      const documents = unread.map((d) => d.key);
      throw FossilError.of(
        'document/unread',
        { documents },
        `${documents.length} document(s) could not be read: ${documents.join(', ')}`,
        { cause: unread[0]!.problem },
      );
    }

    const report = await exec.run(job.host, job.id);
    await job.complete({ status: 'completed', manifest: report });
    return report;
  } catch (e) {
    const failure = isFossilError(e)
      ? e
      : FossilError.of('internal/bug', { what: 'the run failed outside fossil' }, 'internal error: the run failed outside fossil', {
          cause: e,
        });
    await job.complete({ status: 'failed', problem: failure.problem }).catch(() => {});
    throw failure;
  } finally {
    exec?.free();
  }
}
