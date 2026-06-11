/**
 * End-to-end browser-driven job orchestration: resolve a program's sources,
 * fetch them by signed URL, run the mapping on DataFusion-WASM, upload the
 * GraphAr output by signed PUT, and report completion. The host's HTTP calls
 * (signing, completion) are injected via {@link JobTransport}, so this stays
 * framework- and server-agnostic — keasy wires it to its job endpoints.
 */
import type {
  ConnectionRefs,
  ExecutorResult,
  FossilExecutor,
  RunStatus,
  SourceInput,
} from './index.js';

/**
 * The host calls {@link runJob} needs. keasy implements these against its job
 * API (`GET source-refs`, `POST sources/urls`, `POST output/urls`,
 * `PATCH /v1/jobs/{id}`); injected so the orchestration carries no server coupling.
 */
export interface JobTransport {
  /** The connection ref-map `{ name: baseUrl }` for resolving `@conn` aliases. */
  sourceRefs(): Promise<ConnectionRefs>;
  /** Sign GET URLs for the (resolved) source URIs → `{ uri: fetchUrl }`. */
  signSourceUrls(uris: string[]): Promise<Record<string, string>>;
  /** Sign PUT URLs for the output keys → `{ path: putUrl }`. */
  signOutputUrls(paths: string[]): Promise<Record<string, string>>;
  /** Report the run outcome (PATCH the job). */
  complete(req: CompletePayload): Promise<void>;
}

/** The completion payload the host PATCHes back. */
export interface CompletePayload {
  status: 'completed' | 'failed';
  manifest?: RunStatus;
  error?: string;
}

/** Options for {@link runJob}. */
export interface RunJobOptions {
  /** Output dataset dest label (the server overrides it authoritatively). */
  dest?: string;
  /** Output `ShEx` schema text, if any. */
  shex?: string;
  /** Byte fetch/upload impl (defaults to global `fetch`) — override in tests. */
  fetchImpl?: typeof fetch;
}

/**
 * Run a fossil mapping in the browser, end-to-end. Returns the `RunStatus` on
 * success; on any failure reports a `failed` completion (best-effort) and
 * rethrows. Does NOT `free()` the executor — the caller owns its lifecycle.
 */
export async function runJob(
  exec: FossilExecutor,
  program: string,
  transport: JobTransport,
  opts: RunJobOptions = {},
): Promise<RunStatus> {
  const doFetch = opts.fetchImpl ?? fetch;
  try {
    // 1. Resolve `@conn` aliases, then enumerate what to fetch.
    const refs = await transport.sourceRefs();
    const descriptors = exec.sources(program, refs, opts.shex);

    // 2. Sign + fetch each source's bytes (signed GET for cloud, verbatim else).
    const uris = descriptors.map((d) => d.uri);
    const signed = uris.length ? await transport.signSourceUrls(uris) : {};
    const sources: SourceInput[] = await Promise.all(
      descriptors.map(async (d) => {
        const res = await doFetch(signed[d.uri] ?? d.uri);
        if (!res.ok) throw new Error(`fetch source ${d.uri}: ${res.status}`);
        return { uri: d.uri, format: d.format, bytes: new Uint8Array(await res.arrayBuffer()) };
      }),
    );

    // 3. Execute on DataFusion-WASM → GraphAr files + RunStatus.
    const { files, runStatus }: ExecutorResult = await exec.run(
      program,
      sources,
      opts.dest ?? '',
      refs,
      opts.shex,
    );

    // 4. Sign + signed-PUT each output file directly to cloud.
    const putUrls = await transport.signOutputUrls(files.map((f) => f.path));
    await Promise.all(
      files.map(async (f) => {
        const url = putUrls[f.path];
        if (!url) throw new Error(`no signed PUT URL for ${f.path}`);
        const res = await doFetch(url, {
          method: 'PUT',
          body: f.bytes as BodyInit,
          headers: { 'Content-Type': 'application/octet-stream' },
        });
        if (!res.ok) throw new Error(`upload ${f.path}: ${res.status}`);
      }),
    );

    // 5. Report success (server stamps the authoritative dest).
    await transport.complete({ status: 'completed', manifest: runStatus });
    return runStatus;
  } catch (e) {
    const error = e instanceof Error ? e.message : String(e);
    await transport.complete({ status: 'failed', error }).catch(() => {});
    throw e;
  }
}
