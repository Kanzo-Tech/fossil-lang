import { describe, expect, it } from 'vitest';

import { ident } from '../guards/duck.mjs';

describe('the guards’ SQL', () => {
  // A column name comes from the manifest, which a stranger wrote: it is quoted, never pasted.
  it('delimits a manifest column name as an identifier', () => {
    expect(ident('dense_id')).toBe('"dense_id"');
    expect(ident('a"b')).toBe('"a""b"');
  });
});
