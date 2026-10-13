/** A failure as data and back — what crosses a worker's `postMessage`. */
import { describe, expect, it } from 'vitest';

import { FossilError, isFossilError } from '../src/error.js';
import { failureOf, rebuilt } from '../src/failure.js';

/** Through a structured clone, as a `postMessage` sends it. */
const crossed = (e: unknown): Error => rebuilt(structuredClone(failureOf(e)));

describe('a failure crosses a structured clone', () => {
  it('keeps a FossilError whole: code, data and detail', () => {
    const e = FossilError.of('storage/host-refused', { scope: 'job j' }, { cause: new Error('no') });
    const back = crossed(e);
    expect(isFossilError(back, 'storage/host-refused')).toBe(true);
    expect((back as FossilError).data).toEqual({ scope: 'job j' });
    expect(back.message).toBe(e.message);
  });

  it("keeps a host's own code, title and data as own properties", () => {
    const refusal = Object.assign(new Error('the store refused'), {
      name: 'ApiError',
      code: 'store/refused',
      title: 'Refused',
      data: { status: 403 },
    });
    const back = crossed(refusal) as Error & Record<string, unknown>;
    expect(back.name).toBe('ApiError');
    expect(back.message).toBe('the store refused');
    expect(back.code).toBe('store/refused');
    expect(back.title).toBe('Refused');
    expect(back.data).toEqual({ status: 403 });
  });

  it('turns a thrown value that is no object into a message', () => {
    expect(crossed('plain').message).toBe('plain');
  });
});
