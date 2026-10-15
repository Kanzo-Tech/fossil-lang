/**
 * The run's worker and the page, both halves of `src/protocol.ts`, over a real `MessageChannel`:
 * what keasy's own worker asserted, now that the worker is fossil's (`/docs/design/off-the-page`).
 * The executor itself is not in it — `run.test.ts` runs that, in this thread, as Node does.
 */
import { MessageChannel } from 'node:worker_threads';

import { FossilError, isFossilError, type Host } from '@fossil-lang/types';
import { describe, expect, it } from 'vitest';

import { drive, serve, type Port, type Request, type Runner } from '../src/protocol.js';

const REQUEST: Request = { type: 'run', program: 'p', into: 'memory', host: true };

/** Both halves over one channel; `terminated` says whether the page ended the worker. */
function wired(runner: Runner, host: Host, signal?: AbortSignal) {
  const { port1: page, port2: worker } = new MessageChannel();
  const state = { terminated: false };
  let listener: ((message: string) => void) | undefined;
  serve(worker as unknown as Port, runner);
  const answer = drive(page as unknown as Port, REQUEST, {
    host,
    signal,
    terminate: () => {
      state.terminated = true;
      page.close();
    },
    crashes: (l) => (listener = l),
  });
  return { answer, state, crash: (message: string) => listener?.(message) };
}

const refusing: Host = {
  connections: async () => ({}),
  credentials: async () => {
    throw Object.assign(new Error('the store refused'), { name: 'ApiError', code: 'store/refused', data: { status: 403 } });
  },
};

describe('a run in a worker', () => {
  it("answers the run's answer, the files' bytes with it", async () => {
    const bytes = new Uint8Array([1, 2, 3]);
    const { answer, state } = wired(async () => ({ report: { location: 'memory://', dropped: [] }, files: [{ path: 'fossil.graph.yml', bytes }] }), refusing);
    const result = (await answer) as { files: { path: string; bytes: Uint8Array }[] };
    expect(Array.from(result.files[0]!.bytes)).toEqual([1, 2, 3]);
    expect(state.terminated, 'the worker is ended with its run').toBe(true);
  });

  it('asks the host on the page, and its answer reaches the run', async () => {
    const host: Host = { connections: async () => ({ lake: 's3://lake' }), credentials: async () => [] };
    const { answer } = wired(async (_request, h) => h!.connections({ signal: new AbortController().signal }), host);
    expect(await answer).toEqual({ lake: 's3://lake' });
  });

  it("keeps a host refusal's own code through the worker, under the run's failure", async () => {
    const { answer } = wired(async (_request, h) => {
      try {
        return await h!.credentials({ job: 'j' }, 'write', { signal: new AbortController().signal });
      } catch (cause) {
        // What fossil does with a refusal: its own code, the host's error as the cause.
        throw FossilError.of('storage/host-refused', { scope: 'job j' }, { cause });
      }
    }, refusing);
    const failure = await answer.catch((e: unknown) => e);
    expect(isFossilError(failure, 'storage/host-refused')).toBe(true);
    expect((failure as FossilError).problem.cause).toMatchObject({ name: 'ApiError', code: 'store/refused', data: { status: 403 } });
  });

  it('a failure that is no FossilError crosses with its name, message and code', async () => {
    const { answer } = wired(async (_request, h) => h!.credentials({ job: 'j' }, 'write', { signal: new AbortController().signal }), refusing);
    await expect(answer).rejects.toMatchObject({ name: 'ApiError', message: 'the store refused', code: 'store/refused' });
  });

  it('an abort ends the worker at once, rejects with the reason, and aborts the host call in flight', async () => {
    const stop = new AbortController();
    let inFlight: AbortSignal | undefined;
    const waiting: Host = {
      connections: async () => ({}),
      credentials: (_scope, _access, { signal }) => {
        inFlight = signal;
        return new Promise(() => {});
      },
    };
    const { answer, state } = wired(
      async (_request, h) => h!.credentials({ job: 'j' }, 'write', { signal: new AbortController().signal }),
      waiting,
      stop.signal,
    );
    await new Promise((next) => setTimeout(next, 20));
    expect(inFlight?.aborted).toBe(false);
    stop.abort(new DOMException('stopped', 'AbortError'));
    await expect(answer).rejects.toMatchObject({ name: 'AbortError' });
    expect(state.terminated).toBe(true);
    expect(inFlight?.aborted).toBe(true);
  });

  it('a worker that dies before it started is module/unreachable, and after, internal/bug', async () => {
    const hung: Runner = () => new Promise(() => {});
    // `ready` crosses before the crash, so this one had started.
    const started = wired(hung, refusing);
    await new Promise((next) => setTimeout(next, 20));
    started.crash('RuntimeError: unreachable');
    await expect(started.answer).rejects.toMatchObject({ name: 'FossilError', code: 'internal/bug' });

    const { port1: page } = new MessageChannel();
    let listener: ((message: string) => void) | undefined;
    const never = drive(page as unknown as Port, REQUEST, { host: refusing, terminate: () => page.close(), crashes: (l) => (listener = l) });
    listener!('Failed to fetch worker.js');
    await expect(never).rejects.toMatchObject({ name: 'FossilError', code: 'module/unreachable' });
  });
});
