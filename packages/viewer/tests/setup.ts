/**
 * Test setup for @fossil-lang/viewer (Phase 12).
 *
 * happy-dom polyfills + React act flag + WebGL stub. Cosmos.gl boots
 * a WebGL2 context; happy-dom returns null from `getContext('webgl2')`
 * by default, which is exactly the behaviour our TabularFallback path
 * expects in plan 12-04. We make the null-return explicit here so
 * Cosmos.gl's mount path can be exercised by unit tests without ever
 * touching a real GPU.
 *
 * The Mosaic Selection mock + Cosmos Graph constructor stubs live in
 * per-test vi.mock() calls (plans 12-02 / 12-03 install them at
 * describe-scope to keep them locally relevant).
 */
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

// React 18+ act() environment flag — required for @testing-library/react
// v16 + happy-dom interop.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// Stub canvas getContext to always return null in unit tests. This is the
// no-WebGL path Cosmos.gl will throw on, which the viewer's fallback path
// will catch and render TabularFallback. Tests targeting the WebGL mount
// path can override this stub per-test if needed.
if (typeof HTMLCanvasElement !== 'undefined') {
  HTMLCanvasElement.prototype.getContext = function getContextStub(): null {
    return null;
  } as typeof HTMLCanvasElement.prototype.getContext;
}

afterEach(() => {
  cleanup();
});
