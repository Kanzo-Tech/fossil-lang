import { describe, it, expect } from 'vitest';
import { VIEWER_PACKAGE_VERSION } from '../src/index.js';

describe('@fossil-lang/viewer scaffold', () => {
  it('exports a package version constant', () => {
    expect(VIEWER_PACKAGE_VERSION).toBe('0.1.0');
  });
});
