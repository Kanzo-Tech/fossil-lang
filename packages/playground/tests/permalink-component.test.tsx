/**
 * Component-level usePermalink tests (PLAY-04).
 *
 * Why test the hook directly (not the full <FossilPlayground/>):
 *   - The hook IS the wiring contract per CONTEXT.md locked decision.
 *     The component just delegates to it.
 *   - Rendering <FossilPlayground/> spins up CodeMirror + LSP-Worker
 *     stubs + DuckDB stubs — none of which are necessary to exercise
 *     the decode + debounced-encode invariants. renderHook from
 *     @testing-library/react gives us the smallest possible surface.
 *   - Production end-to-end behaviour is exercised by the Playwright
 *     spec in apps/landing/tests/e2e/permalink.spec.ts — which loads
 *     the real page, hits the real component, and walks the real URL
 *     fragment loop.
 *
 * Three invariants:
 *   1) initialPermalink decodes + calls onHydrate exactly once.
 *   2) Corrupted initialPermalink calls onError (not throw) + does NOT
 *      call onHydrate. This is the "don't crash the host" guarantee.
 *   3) State changes trigger onStateChange after the debounce window —
 *      coalesces multiple rapid changes into a single emit.
 */

import { describe, test, expect, vi, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { usePermalink } from '../src/hooks/usePermalink.js';
import { encode as encodePermalink } from '../src/permalink/index.js';

describe('usePermalink (PLAY-04)', () => {
  afterEach(() => {
    // Defensive: tests that opt into fake timers MUST restore real ones
    // so the next test's render doesn't hang on a never-firing timer.
    vi.useRealTimers();
  });

  test('initialPermalink decodes + calls onHydrate exactly once', () => {
    const permalink = encodePermalink({
      source: 'prefix ex: <https://example.org/>\n',
      csvw: '{"@context":"http://www.w3.org/ns/csvw"}',
    });
    const onHydrate = vi.fn();
    const onError = vi.fn();

    const { rerender } = renderHook(
      (args: { initialPermalink: string | undefined }) =>
        usePermalink({
          initialPermalink: args.initialPermalink,
          source: '',
          csvw: undefined,
          shex: undefined,
          onHydrate,
          onError,
        }),
      { initialProps: { initialPermalink: permalink } },
    );

    expect(onError).not.toHaveBeenCalled();
    expect(onHydrate).toHaveBeenCalledTimes(1);
    expect(onHydrate.mock.calls[0]?.[0]).toMatchObject({
      v: 1,
      source: 'prefix ex: <https://example.org/>\n',
      csvw: '{"@context":"http://www.w3.org/ns/csvw"}',
    });

    // Even if the parent re-passes the permalink (or a different one), the
    // hydratedRef latch must keep onHydrate at exactly one call. This is
    // the "don't clobber the user's in-progress edits" guarantee.
    rerender({ initialPermalink: permalink });
    rerender({ initialPermalink: undefined });
    expect(onHydrate).toHaveBeenCalledTimes(1);
  });

  test('corrupted initialPermalink calls onError instead of throwing', () => {
    const onError = vi.fn();
    const onHydrate = vi.fn();

    // 'not-a-valid-permalink' fails at the gzip-ungzip step (base64url
    // decode succeeds for the ASCII chars but the resulting bytes aren't
    // valid gzip). The hook MUST swallow the throw and surface via
    // onError — the host cannot crash on a user-shared bad URL.
    expect(() => {
      renderHook(() =>
        usePermalink({
          initialPermalink: 'not-a-valid-permalink',
          source: '',
          csvw: undefined,
          shex: undefined,
          onHydrate,
          onError,
        }),
      );
    }).not.toThrow();

    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0]?.[0]).toBeInstanceOf(Error);
    expect(onHydrate).not.toHaveBeenCalled();
  });

  test('late-arriving initialPermalink (undefined → string) still hydrates', () => {
    // Regression guard for the Next.js dynamic({ ssr: false }) + useEffect
    // host pattern: PlaygroundHost reads window.location.hash in a
    // useEffect, so on first paint the component sees
    // `initialPermalink=undefined`. The hook MUST defer latching until it
    // actually sees a permalink — otherwise the second-render flip would
    // be skipped and the host would never hydrate from the URL fragment.
    const permalink = encodePermalink({
      source: 'late = "hydration\\nstill works"\n',
    });
    const onHydrate = vi.fn();

    const { rerender } = renderHook(
      (args: { initialPermalink: string | undefined }) =>
        usePermalink({
          initialPermalink: args.initialPermalink,
          source: '',
          csvw: undefined,
          shex: undefined,
          onHydrate,
        }),
      { initialProps: { initialPermalink: undefined } },
    );

    expect(onHydrate).not.toHaveBeenCalled();

    // Second render brings the actual permalink — this is the
    // useEffect-after-first-paint case the Playwright spec exercises
    // end-to-end via window.location.hash.
    rerender({ initialPermalink: permalink });
    expect(onHydrate).toHaveBeenCalledTimes(1);
    expect(onHydrate.mock.calls[0]?.[0]).toMatchObject({
      v: 1,
      source: 'late = "hydration\\nstill works"\n',
    });
  });

  test('source change triggers debounced onStateChange', () => {
    vi.useFakeTimers();
    const onStateChange = vi.fn();
    const onError = vi.fn();

    const { rerender } = renderHook(
      (args: { src: string }) =>
        usePermalink({
          initialPermalink: undefined,
          source: args.src,
          csvw: undefined,
          shex: undefined,
          onStateChange,
          onError,
          debounceMs: 100,
        }),
      { initialProps: { src: 'a' } },
    );

    // The initial render schedules an emit but the timer hasn't elapsed.
    // Three quick changes must coalesce into exactly one emit after the
    // debounce window.
    rerender({ src: 'ab' });
    rerender({ src: 'abc' });
    expect(onStateChange).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(150);
    });

    expect(onError).not.toHaveBeenCalled();
    expect(onStateChange).toHaveBeenCalledTimes(1);
    const encoded = onStateChange.mock.calls[0]?.[0];
    expect(typeof encoded).toBe('string');
    expect((encoded as string).length).toBeGreaterThan(0);
    // base64url charset (no '+', '/', '=')
    expect(encoded).toMatch(/^[A-Za-z0-9_-]+$/);
  });
});
