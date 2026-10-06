import { describe, expect, it } from 'vitest';

import { FossilError, rowOf } from '../src/index.js';

describe('rowOf', () => {
  // The three hosts once built this row by hand and disagreed on the range: an empty range at
  // 0:0 draws nothing in some editors.
  it('puts a problem with no place on the first character', () => {
    const { problem } = FossilError.of('engine/failed', {}, { help: 'retry' });
    const row = rowOf('main.fossil', problem, 'x: unread');
    expect(row.range).toEqual({ start: { line: 0, character: 0 }, end: { line: 0, character: 1 } });
    expect(row).toMatchObject({ code: 'engine/failed', severity: 1, message: 'x: unread', help: 'retry' });
  });
});
