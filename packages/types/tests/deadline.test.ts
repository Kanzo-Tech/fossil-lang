import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { FossilError, isFossilError } from '../src/index.js';
import { MODULE_MS, attachCause, boot, loader, until, within } from '../src/internal.js';

const silent = (after: number) => new Error(`silent after ${after}`);
const never = <T>(): Promise<T> => new Promise<T>(() => {});

describe('within', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('answers what the wait answered, and leaves no timer behind', async () => {
    await expect(within(1_000, async () => 7, { silent })).resolves.toBe(7);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('rejects with what silent builds once the deadline passes, and not a moment before', async () => {
    let handed: AbortSignal | undefined;
    const waiting = within(1_000, (signal) => ((handed = signal), never()), { silent });
    const outcome = waiting.catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(999);
    expect(handed?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(await outcome).toEqual(new Error('silent after 1000'));
    expect(handed?.aborted).toBe(true);
  });

  it("rejects with the caller's reason when the caller stops first", async () => {
    const stop = new AbortController();
    const waiting = within(1_000, () => never(), { signal: stop.signal, silent });
    stop.abort(new DOMException('stopped', 'AbortError'));
    await expect(waiting).rejects.toMatchObject({ name: 'AbortError' });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('does not start a wait the caller already stopped', async () => {
    const wait = vi.fn(async () => 1);
    await expect(within(1_000, wait, { signal: AbortSignal.abort(), silent })).rejects.toMatchObject({
      name: 'AbortError',
    });
    expect(wait).not.toHaveBeenCalled();
  });
});

describe('until', () => {
  it('abandons the wait, not the work, and a late rejection is not unhandled', async () => {
    const stop = new AbortController();
    let fail!: (e: unknown) => void;
    const work = new Promise<number>((_, reject) => (fail = reject));
    const waiting = until(work, stop.signal);
    stop.abort(new DOMException('stopped', 'AbortError'));
    await expect(waiting).rejects.toMatchObject({ name: 'AbortError' });
    fail(new Error('late'));
    await expect(work).rejects.toThrow('late');
  });
});

describe('boot', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('is module/unreachable for a module that could not be fetched, the browser’s error its cause', async () => {
    const dropped = new TypeError('Failed to fetch');
    const e = await boot('m_bg.wasm', () => Promise.reject(dropped)).catch((x: unknown) => x);
    expect(isFossilError(e, 'module/unreachable')).toBe(true);
    expect((e as FossilError<'module/unreachable'>).data).toEqual({ location: 'm_bg.wasm' });
    expect((e as Error).cause).toBe(dropped);
  });

  it('is internal/bug for a module that would not compile', async () => {
    const broken = Object.assign(new Error('bad magic'), { name: 'CompileError' });
    const e = await boot('m_bg.wasm', () => Promise.reject(broken)).catch((x: unknown) => x);
    expect(isFossilError(e, 'internal/bug')).toBe(true);
    expect((e as Error).cause).toBe(broken);
  });

  it('gives up on a module that never arrives after 60 s, as module/unreachable with after', async () => {
    const outcome = boot('m_bg.wasm', never).catch((x: unknown) => x);
    await vi.advanceTimersByTimeAsync(MODULE_MS);
    const e = await outcome;
    expect(isFossilError(e, 'module/unreachable')).toBe(true);
    expect((e as FossilError<'module/unreachable'>).data).toEqual({ location: 'm_bg.wasm', after: MODULE_MS });
    expect(((e as Error).cause as Error).name).toBe('TimeoutError');
  });
});

describe('loader', () => {
  it('keeps a boot that succeeded and forgets one that failed', async () => {
    const init = vi.fn<(options?: { module_or_path: string }) => Promise<unknown>>();
    init.mockRejectedValueOnce(new TypeError('Failed to fetch')).mockResolvedValue('ok');
    const load = loader<string>('m_bg.wasm', init);
    await expect(load('bytes')).rejects.toSatisfy((e) => isFossilError(e, 'module/unreachable'));
    await expect(load()).resolves.toBe('ok');
    await expect(load()).resolves.toBe('ok');
    expect(init.mock.calls).toEqual([[{ module_or_path: 'bytes' }], [undefined]]);
  });
});

describe('attachCause', () => {
  it('appends the cleanup failure at the end of the chain, replacing nothing', () => {
    const engine = new Error('engine said no');
    const failure = FossilError.of('engine/failed', {}, { cause: engine });
    const cleanup = new Error('DROP SECRET failed');
    expect(attachCause(failure, cleanup)).toBe(failure);
    expect(failure.cause).toBe(engine);
    expect(engine.cause).toBe(cleanup);
  });

  it('carries it into the problem when the last error is a FossilError', () => {
    const inner = FossilError.of('storage/no-credential', { scope: 'job j', access: 'read' });
    const outer = FossilError.of('corpus/unreadable', { path: 'fossil.json' }, { cause: inner });
    attachCause(outer, new Error('release failed'));
    expect(inner.cause).toEqual(new Error('release failed'));
    const wire = outer.problem.cause;
    expect(wire !== undefined && 'code' in wire && wire.cause).toEqual({ name: 'Error', detail: 'release failed' });
  });
});
