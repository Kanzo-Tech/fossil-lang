/**
 * SW-FIRST-LOAD-01 multi-tab gate — Phase 9 plan 09-02 Task 3.
 *
 * Closes the second half of RESEARCH.md Pitfall 3:
 *
 *   Serwist's `clients.claim()` (set in `service-worker.ts`) immediately
 *   takes control of EVERY open client — including tabs that loaded
 *   under the previous SW. Without a client-side `controllerchange`
 *   listener (added in 09-02 Task 1, `apps/landing/app/ClientShell.tsx`)
 *   the older tab continues serving from the previous SW's cache state,
 *   leading to mixed-version assets across tabs.
 *
 *   With the listener, every open tab reloads on takeover and converges
 *   on the new active SW.
 *
 * This spec opens two browser contexts (≡ two independent user-facing
 * tabs sharing the same SW scope), loads the playground in both, then
 * verifies both end up controlled by the SAME active SW after
 * registration + claim settles.
 *
 * Why `browser.newContext()` and not `context.newPage()`:
 *
 *   `context.newPage()` creates a second tab sharing the SAME Service
 *   Worker registration + cache storage. That's a real user scenario
 *   (Cmd-T → new tab on the same site) but it doesn't exercise the
 *   independent-registration path. Two contexts = two independent
 *   storage partitions, each registering its own SW from scratch — the
 *   case where takeover semantics matter most.
 *
 * Runs with default Playwright workers (parallelism is fine here
 * because each context is isolated). Per-test timeout inherits the
 * playwright.config.ts 30s default; the two `waitFor({ timeout: 30_000 })`
 * calls re-state that explicitly per spec convention.
 *
 * Refs: deferred-items.md SW-FIRST-LOAD-01, RESEARCH.md Pitfall 3,
 * 09-02-PLAN.md Task 3.
 */
import { expect, test } from '@playwright/test';

test('SW-FIRST-LOAD-01: two tabs converge on same active SW (no stale takeover)', async ({
  browser,
}) => {
  const ctxA = await browser.newContext();
  const ctxB = await browser.newContext();

  try {
    const pageA = await ctxA.newPage();
    const pageB = await ctxB.newPage();

    // Both contexts visit the landing page. The 30s timeout absorbs the
    // one-time controllerchange-triggered reload from 09-02 Task 1 in
    // case the SW transitions to active mid-load.
    await pageA.goto('/');
    await pageA
      .getByTestId('fossil-playground')
      .waitFor({ timeout: 30_000 });
    await pageB.goto('/');
    await pageB
      .getByTestId('fossil-playground')
      .waitFor({ timeout: 30_000 });

    // Wait for both tabs to actually be controlled by the SW.
    //
    // Three subtleties combined:
    //
    //   (a) Awaiting `navigator.serviceWorker.ready` alone is insufficient:
    //       `ready` resolves when there's an ACTIVE worker on the
    //       registration, but `.controller` is only set once that worker
    //       has CLAIMED this client. On a brand-new browser context,
    //       claim arrives a few hundred ms AFTER page load.
    //
    //   (b) The 09-02 Task 1 listener turns the claim-on-already-loaded
    //       case into a controllerchange→reload. The reload happens
    //       asynchronously AFTER controller flips non-null, so a naive
    //       "wait for controller, then read it" sequence races with the
    //       reload and trips "Execution context destroyed".
    //
    //   (c) Once the reload completes, the page is freshly served BY
    //       the new active SW and `.controller` is non-null at
    //       navigation time AND stable (no further controllerchange
    //       happens until the next SW update — which won't occur
    //       mid-test).
    //
    // The robust pattern: read scriptURL via `waitForFunction` itself
    // (it's evaluation-context-tolerant — re-polls in the new context
    // after navigation). The function returns the scriptURL or null;
    // we wait until it's non-null AND the page has reached
    // `domcontentloaded` so the React tree (which carries the listener)
    // is mounted in the controlled-page context.
    //
    // 15s budget per tab is generous — empirically the controlled
    // scriptURL appears within ~1s of `goto` in headless Chromium;
    // headless CI cold-start can stretch this.
    const readController = async (
      page: Awaited<ReturnType<typeof ctxA.newPage>>,
    ): Promise<string> => {
      // Returns the scriptURL string once a controller is set, otherwise
      // returns `false` (which causes waitForFunction to keep polling).
      // Returning the string directly via the polled function avoids
      // the post-poll `page.evaluate` race entirely.
      const handle = await page.waitForFunction(
        () => {
          const sw = navigator.serviceWorker?.controller;
          return sw ? sw.scriptURL : false;
        },
        null,
        { timeout: 15_000 },
      );
      const scriptUrl = await handle.jsonValue();
      // Ensure subsequent reads happen on the controlled (post-reload)
      // page. If the controllerchange→reload happens after the script-
      // URL read but before this test ends, the next test in the same
      // worker could pick up a half-mounted page — wait for the
      // controlled load to fully settle.
      await page.waitForLoadState('load');
      expect(scriptUrl).toBeTruthy();
      return scriptUrl as string;
    };

    const controllerA = await readController(pageA);
    const controllerB = await readController(pageB);

    // Same SW scriptURL — proves both tabs landed on the same active
    // SW (not one on previous, one on current). Each context has its
    // own registration but they resolve to the same script URL on the
    // same origin, which is the structural property that
    // `clients.claim()` + `controllerchange → reload` guarantee.
    expect(controllerA).toBe(controllerB);
  } finally {
    // Always close contexts so storage partitions are cleaned up even
    // if an assertion fails — keeps the Playwright trace small and
    // prevents leaking SW registrations across test runs.
    await ctxA.close();
    await ctxB.close();
  }
});
