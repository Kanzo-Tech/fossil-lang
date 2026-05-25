/**
 * usePermalink — PLAY-04 wiring hook.
 *
 * Bridges the standalone permalink encoder/decoder (plan 09-03, see
 * ../permalink/index.ts) into the React component lifecycle, without touching
 * `window.location`. The host (apps/landing/app/PlaygroundHost.tsx) is
 * responsible for the URL-fragment side of the loop — read on mount, write on
 * onStateChange — per the CONTEXT.md locked decision:
 *
 *   "<FossilPlayground/> accepts `initialPermalink?: string`; emits
 *    `onStateChange?: (permalink: string) => void`. The component does NOT
 *    touch `window.location` directly — host (`apps/landing/`) wires the URL
 *    fragment."
 *
 * Two effects:
 *   1) One-shot DECODE on mount. If `initialPermalink` is provided, attempt to
 *      decode it. On success call `onHydrate` once with the decoded shape so
 *      the component can seed its editor + csvw + shex state. On failure
 *      (corrupted, future-version, base64/gzip/JSON errors) call `onError`
 *      and continue with the component's defaults — NEVER crash the host.
 *      The `hydratedRef` latch guarantees a single hydration even if the
 *      `initialPermalink` reference identity changes (e.g. host re-renders).
 *
 *   2) Debounced ENCODE on every state change. Whenever (source, csvw, shex)
 *      change, schedule an encode + `onStateChange` callback. Debounce (default
 *      200ms) prevents hammering the URL on every keystroke — the host's
 *      `history.replaceState` is cheap but pointless to call hundreds of times
 *      a second. If `encode` throws `PermalinkTooLargeError`, surface to
 *      `onError` but do NOT block — the user keeps typing; the host can show a
 *      "share via Gist" affordance.
 *
 * Pure React, no DOM globals. Testable via `renderHook` from
 * @testing-library/react against the happy-dom environment.
 */

import { useEffect, useRef } from 'react';
import {
  encode as encodePermalink,
  decode as decodePermalink,
  PermalinkTooLargeError,
  type PermalinkStateV1,
} from '../permalink/index.js';

export interface UsePermalinkArgs {
  /**
   * The permalink string to hydrate from on mount. If `undefined`, no
   * hydration is attempted. Read by the host from `window.location.hash`
   * (without the leading `#`).
   */
  initialPermalink: string | undefined;

  /**
   * Current editor source — first positional state in the envelope. Required
   * because the envelope always carries a `source` field (even if empty).
   */
  source: string;

  /** Current CSVW descriptor (optional — emitted in envelope iff defined). */
  csvw: string | undefined;

  /** Current ShEx target shape (optional — emitted iff defined). */
  shex: string | undefined;

  /**
   * Called exactly once on the first successful decode of `initialPermalink`.
   * The component implements this to setState the editor/csvw/shex slots from
   * the decoded shape. If decode fails this is NOT called.
   */
  onHydrate?: (state: PermalinkStateV1) => void;

  /**
   * Called (debounced) with the freshly-encoded permalink whenever (source,
   * csvw, shex) change. The host typically pipes this into
   * `window.history.replaceState(null, '', '#' + permalink)`.
   */
  onStateChange?: (permalink: string) => void;

  /**
   * Called with either a decode error (corrupted/future-version permalink) or
   * a `PermalinkTooLargeError` from encode. Hosts typically log to console +
   * optionally render a one-time banner. Defaults to a console.warn no-op via
   * the consumer site.
   */
  onError?: (err: Error) => void;

  /**
   * Debounce window for the encode + emit cycle. Default 200 ms — long enough
   * to coalesce typing bursts, short enough that "share my URL right now" UX
   * stays snappy. Tests override to 100 ms to keep the suite fast.
   */
  debounceMs?: number;
}

export function usePermalink(args: UsePermalinkArgs): void {
  const {
    initialPermalink,
    source,
    csvw,
    shex,
    onHydrate,
    onStateChange,
    onError,
    debounceMs = 200,
  } = args;

  // --- One-shot decode on mount ----------------------------------------
  // `hydratedRef` survives across re-renders so even if the parent passes a
  // new `initialPermalink` reference (or eventually clears it after wiring),
  // we never double-hydrate and clobber the user's in-progress edits.
  const hydratedRef = useRef(false);
  useEffect(() => {
    if (hydratedRef.current) return;
    hydratedRef.current = true;
    if (!initialPermalink) return;
    try {
      const state = decodePermalink(initialPermalink);
      onHydrate?.(state);
    } catch (e) {
      onError?.(e as Error);
    }
    // Only the initialPermalink + handler identities matter on first paint.
    // Subsequent identity flips MUST NOT re-trigger hydration — that's the
    // hydratedRef latch above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialPermalink]);

  // --- Debounced encode + emit on state change -------------------------
  // The timer ref lets the cleanup function clear any pending emit so we
  // don't fire a stale state after unmount (or after a fast follow-up change
  // supersedes a still-pending emit).
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (!onStateChange) return;
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      try {
        const encoded = encodePermalink({ source, csvw, shex });
        onStateChange(encoded);
      } catch (e) {
        if (e instanceof PermalinkTooLargeError) {
          onError?.(e);
        } else {
          onError?.(e as Error);
        }
      }
    }, debounceMs);
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [source, csvw, shex, onStateChange, onError, debounceMs]);
}
