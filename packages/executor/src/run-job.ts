/**
 * End-to-end browser-driven job orchestration: read the documents the program
 * names, fetch its sources, run the mapping on DataFusion-WASM, upload the
 * GraphAr output by signed PUT, and report completion.
 *
 * The read half is the {@link SourceHost} every `@fossil-lang/*` package takes;
 * the write half, {@link JobOutput}, is the job's own. Both are injected, so
 * this carries no server coupling — keasy wires them to its job endpoints.
 */
import { resolveDocuments, type SourceHost } from '@fossil-lang/types';

import { FossilExecutor } from './client.js';
import type { ExecutorResult, RunReport, SourceInput } from './index.js';

/** Where a job's output goes: signed PUT URLs for its files, and its outcome. */
export interface JobOutput {
  /** Sign PUT URLs for the output keys → `{ path: putUrl }`. */
  signOutputUrls(paths: string[]): Promise<Record<string, string>>;
  /** Report the run outcome (PATCH the job). */
  complete(req: CompletePayload): Promise<void>;
}

/** What {@link runJob} reads through and writes to. */
export interface Job {
  host: SourceHost;
  output: JobOutput;
}

/** The completion payload the host PATCHes back. */
export interface CompletePayload {
  status: 'completed' | 'failed';
  /**
   * The manifest of what was uploaded. The field was already called this and
   * carried a `RunStatus` that was not one; it is the manifest now.
   */
  manifest?: RunReport;
  error?: string;
}

/** Options for {@link runJob}. */
export interface RunJobOptions {
  /** Output dataset dest label (the server overrides it authoritatively). */
  dest?: string;
  /** Byte fetch/upload impl (defaults to global `fetch`) — override in tests. */
  fetchImpl?: typeof fetch;
}

/**
 * Run a fossil mapping in the browser, end-to-end. Returns the `RunReport` on
 * success; on any failure — a document or source that could not be read
 * included — reports a `failed` completion (best-effort) and rethrows.
 *
 * `initFossilExecutor` must have resolved first.
 */
export async function runJob(
  program: string,
  job: Job,
  opts: RunJobOptions = {},
): Promise<RunReport> {
  const doFetch = opts.fetchImpl ?? fetch;
  let exec: FossilExecutor | undefined;
  try {
    exec = new FossilExecutor(program);

    const { unread } = await resolveDocuments(exec, job.host, doFetch);
    if (unread.length > 0) {
      const which = unread.map((d) => `${d.key} (${d.reason})`).join(', ');
      throw new Error(`documents the program names could not be read: ${which}`);
    }

    const descriptors = exec.sources();
    const signed = descriptors.length ? await job.host.sign(descriptors.map((d) => d.uri)) : {};
    const sources: SourceInput[] = await Promise.all(
      descriptors.map(async (d) => {
        const url = signed[d.uri];
        if (!url) throw new Error(`the host does not sign source ${d.uri}`);
        const res = await doFetch(url);
        if (!res.ok) throw new Error(`fetch source ${d.uri}: ${res.status}`);
        return { uri: d.uri, format: d.format, bytes: new Uint8Array(await res.arrayBuffer()) };
      }),
    );

    const { files, report }: ExecutorResult = await exec.run(sources, opts.dest ?? '');

    const putUrls = await job.output.signOutputUrls(files.map((f) => f.path));
    await Promise.all(
      files.map(async (f) => {
        const url = putUrls[f.path];
        if (!url) throw new Error(`no signed PUT URL for ${f.path}`);
        const res = await doFetch(url, {
          method: 'PUT',
          body: f.bytes as BodyInit,
          headers: {
            // Azure block-blob PUT via SAS REQUIRES `x-ms-blob-type` (else 400
            // MissingRequiredHeader). S3/GCS presigned PUTs ignore the unsigned
            // header, so sending it unconditionally is safe across providers
            // (and works for Azurite / custom Azure endpoints too, not just
            // `*.blob.core.windows.net`).
            'Content-Type': 'application/octet-stream',
            'x-ms-blob-type': 'BlockBlob',
          },
        });
        if (!res.ok) throw new Error(`upload ${f.path}: ${res.status}`);
      }),
    );

    await job.output.complete({ status: 'completed', manifest: report });
    return report;
  } catch (e) {
    const error = e instanceof Error ? e.message : String(e);
    await job.output.complete({ status: 'failed', error }).catch(() => {});
    throw e;
  } finally {
    exec?.free();
  }
}
