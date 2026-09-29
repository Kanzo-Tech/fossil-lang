/**
 * A job, end to end in the browser: read the documents the program names, run the mapping on
 * DataFusion-WASM, write the GraphAr output, report the outcome.
 *
 * Every byte goes through `object_store` stores built from credentials the host vends — `read`
 * per connection the program names, `write` on the job. `DataFusion` reads a source through its
 * store by range requests, and the output is written through the job's, in parts when large.
 */
import { resolveDocuments } from '@fossil-lang/storage';
import type { Host } from '@fossil-lang/types';

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
   * The manifest of what was written. The field was already called this and
   * carried a `RunStatus` that was not one; it is the manifest now.
   */
  manifest?: RunReport;
  error?: string;
}

/**
 * Run a fossil mapping in the browser, end-to-end. Returns the `RunReport` on success; on any
 * failure — a document or source that could not be read included — reports a `failed` completion
 * (best-effort) and rethrows.
 *
 * `initFossilExecutor` must have resolved first.
 */
export async function runJob(program: string, job: Job): Promise<RunReport> {
  let exec: FossilExecutor | undefined;
  try {
    exec = new FossilExecutor(program);

    const { unread } = await resolveDocuments(exec, job.host);
    if (unread.length > 0) {
      const which = unread.map((d) => `${d.key} (${d.reason})`).join(', ');
      throw new Error(`documents the program names could not be read: ${which}`);
    }

    const report = await exec.run(job.host, job.id);
    await job.complete({ status: 'completed', manifest: report });
    return report;
  } catch (e) {
    const error = e instanceof Error ? e.message : String(e);
    await job.complete({ status: 'failed', error }).catch(() => {});
    throw e;
  } finally {
    exec?.free();
  }
}
