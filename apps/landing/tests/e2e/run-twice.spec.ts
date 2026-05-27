/**
 * BUG-01 regression canary — Run twice in a row must produce IDENTICAL results.
 *
 * Why this spec exists (Phase 15 plan 15-01 + REQUIREMENTS.md BUG-01):
 *   v0.1 user report (2026-05-26): mounting `<FossilPlayground/>` on the
 *   landing default mapping (`hello-no-csvw.fossil` per Phase 13 plan 13-05)
 *   and clicking the Run button TWICE in a row does NOT produce two equivalent
 *   result sets. Probable causes investigated by 15-01 plan:
 *
 *     - `useInferredDescriptors.introspectAndRegister` may double-register the
 *       same `source_name` on the second Run (the WASM `FossilPlayground`'s
 *       Salsa store could accumulate a stale revision or surface a duplicate-
 *       registration error).
 *     - DuckDB-WASM connection state could leak across Runs (the runPipeline
 *       calls `db.registerFileBuffer` against virtual names that were
 *       registered in the previous Run too).
 *     - `compileInstance` is component-scope (lazy mint, `updateFile` on
 *       subsequent calls) — if Salsa's incremental revision bump doesn't fire
 *       on a byte-identical mapping, the second compile could return a cached
 *       result that mismatches the freshly-fetched DuckDB state.
 *
 * What this spec asserts:
 *   1. Mount landing default. Wait for editor-mount gate
 *      (`Loading editor…` cleared) the same way `landing-run.spec.ts` does.
 *   2. Click Run. Wait for the canonical `https://example.org/user/{N}` IRI
 *      rows to appear. Capture the first-Run vertex-row count.
 *   3. Click Run AGAIN. Wait for the same IRI rows to be present a second time
 *      (the Output panel re-renders).
 *   4. Assert: same vertex-row count both times. No `role="alert"` error
 *      banner appears after the second Run.
 *
 * Diagnosis aid — if this spec FAILS in CI:
 *   - Check `await page.locator('[role="alert"]').textContent()` for the
 *     concrete error. DuckDB binding errors → look at runPipeline's
 *     virtual-FS registration loop (`registerFileBuffer` may need a guarded
 *     re-registration). LSP/Salsa "stale revision" or duplicate-
 *     `registerInferredDescriptor` errors → look at
 *     `useInferredDescriptors.introspectAndRegister` (it needs idempotent
 *     re-registration, NOT accumulation across Runs).
 *   - The vitest unit-level twin lives at
 *     `packages/playground/tests/runPipeline.run-twice.test.ts` and exercises
 *     the same scenario without the Playwright + WASM stack.
 *
 * Note on tightness: the test counts vertex rows by counting visible
 * `https://example.org/user/...` matches inside `data-testid="fossil-playground"`.
 * The exact count depends on the hello example's fixture (5 users); we assert
 * the second-Run count EQUALS the first-Run count rather than hard-coding 5,
 * so the spec survives an example-fixture change without false failure.
 */
import { expect, test } from '@playwright/test';

test('BUG-01: clicking Run twice in a row produces identical results (no state-leak)', async ({
  page,
}) => {
  await page.goto('/');

  // Same mount + editor-ready gates as landing-run.spec.ts. The
  // <FossilPlayground/> sets wasmReady=true after initFossilWasm resolves;
  // before that the editor area shows "Loading editor…" and clicking Run
  // throws "WASM is still loading".
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });
  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeEnabled();

  const root = page.getByTestId('fossil-playground');
  const irisLocator = root.getByText(/https:\/\/example\.org\/user\/[0-9]+/);

  // ----- First Run -----
  await page.getByRole('button', { name: 'Run mapping' }).click();

  // Wait for SC#1-style first-row appearance (the same 5 s budget). If the
  // first Run already fails, this spec also fails — but the failure mode is
  // different from the BUG-01 second-Run regression. Investigate the SC#1
  // gate first if both this spec and `landing-run.spec.ts` go red together.
  await expect(irisLocator.first()).toBeVisible({ timeout: 5_000 });

  // Capture the first-Run state. `.count()` is the lazy-evaluated number of
  // matched elements at the time of the call — stable so long as we don't
  // race the second click.
  const firstRunCount = await irisLocator.count();
  expect(firstRunCount).toBeGreaterThan(0); // sanity — Run succeeded.

  // Sanity: no error banner after the first Run.
  await expect(page.locator('[role="alert"]')).toHaveCount(0, {
    timeout: 1_000,
  });

  // ----- Second Run -----
  // Click Run a second time. Per BUG-01 hypothesis: the WASM compile instance
  // is reused (lazy mint + `updateFile`), the DuckDB Worker is reused, and
  // `useInferredDescriptors.introspectAndRegister` is called against an
  // already-mounted source-name. If state-leak exists, EITHER:
  //   (a) the second `irisLocator.first()` never appears (vertices reset to []
  //       but never repopulate because runPipeline threw inside Output),
  //   (b) the `role="alert"` banner shows a DuckDB binding error or a
  //       Salsa-stale-revision error, OR
  //   (c) the count differs (some vertices dropped/duplicated).
  await page.getByRole('button', { name: 'Run mapping' }).click();

  // Wait for the second Run to complete. We re-assert the IRI rows are
  // present — the component clears vertices=[] inside handleRun before
  // awaiting runPipeline so a brief intermediate empty state is normal.
  await expect(irisLocator.first()).toBeVisible({ timeout: 5_000 });

  // Allow the React render to settle before counting. Playwright's
  // expect.toHaveCount has its own polling so we don't need a separate
  // sleep; passing the same locator + the captured first-Run count IS the
  // assertion.
  await expect(irisLocator).toHaveCount(firstRunCount, { timeout: 5_000 });

  // No error banner after the second Run either. This is the BUG-01-specific
  // regression check: with the bug present, this is where a DuckDB or
  // Salsa-state error typically surfaces in the assertive alert region.
  await expect(page.locator('[role="alert"]')).toHaveCount(0, {
    timeout: 1_000,
  });
});
