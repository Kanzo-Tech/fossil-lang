import { describe, it, expect } from 'vitest';
import * as ui from '../src/index.js';
import { cx } from '../src/utils/cx.js';

describe('@fossil-lang/ui scaffold', () => {
  it('barrel is importable (initially empty)', () => {
    expect(typeof ui).toBe('object');
  });

  it('cx joins truthy string args with single space', () => {
    expect(cx('a', 'b')).toBe('a b');
    expect(cx('a', false, 'b', null, undefined, '', 'c')).toBe('a b c');
    expect(cx()).toBe('');
  });
});
