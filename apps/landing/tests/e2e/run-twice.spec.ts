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

  // Switch the loaded example from the landing default (`hello-no-csvw`) to
  // the legacy `hello` example. The default's source filename contains a
  // hyphen, and the fossil-codegen `derive_view_name` helper emits
  // `CREATE VIEW hello-no-csvw AS …` — which DuckDB's SQL parser rejects
  // (unquoted identifiers cannot contain hyphens). That codegen-side bug is
  // independent of BUG-01 (it's the SC-1 landing-run.spec.ts regression that
  // surfaced concurrently; see 15-01-SUMMARY.md "Deferred Issues") and lives
  // in `crates/fossil-codegen/src/sql.rs::derive_view_name`. Per
  // 15-CONTEXT.md ("NO toca compiler Rust") we route around it here by
  // loading the hyphen-free `hello` example so the run-twice canary can
  // observe the BUG-01 fix in isolation. The combobox + remount path is the
  // host-controlled re-seed flow documented in PlaygroundHost.tsx; once the
  // codegen bug is closed in a future plan (likely 15-05 or a 16-XX
  // hotfix), this spec can revert to the landing default.
  await page
    .getByRole('combobox', { name: 'Load curated example' })
    .selectOption('hello');
  // Wait for the remount: the example load bumps `remountKey` in
  // PlaygroundHost which tears down + re-mounts <FossilPlayground/>. The
  // remount goes through the editor-ready gate again.
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });
  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeEnabled();

  const root = page.getByTestId('fossil-playground');
  // The post-Run vertex/edge counts surface in the FossilViewer's tab labels
  // (`Vertices (N)` / `Edges (N)`) — these live in the rendered DOM (unlike
  // the IRIs themselves which sit on the WebGL canvas and are not text-
  // selectable). The counts ARE the success signal: if compile + run
  // succeed, the labels flip from `(0)` to `(N>0)`; if BUG-01 throws, the
  // labels stay at `(0)` and a `role="alert"` banner appears.
  const verticesTabLabel = (): Promise<string | null> =>
    root.getByRole('tab', { name: /^Vertices \(\d+\)$/ }).textContent();
  const edgesTabLabel = (): Promise<string | null> =>
    root.getByRole('tab', { name: /^Edges \(\d+\)$/ }).textContent();

  // The error banner the playground renders on Run failure is a *visible*
  // `role="alert"` with the `Error: …` prefix inside `data-testid="fossil-
  // playground"` (see FossilPlayground.tsx — the `<div role="alert">` block
  // gated on `runError || duck.error`). The page-level accessibility tree
  // also surfaces a stray empty live-region "alert" entry (the announce()
  // SR-only region), so the test scopes to the playground tree and asserts
  // on TEXT content rather than element count.
  const playgroundAlerts = root.locator('[role="alert"]');

  // ----- First Run -----
  await page.getByRole('button', { name: 'Run mapping' }).click();

  // Wait for the post-Run state: vertex count flips from `(0)` to `(N>0)`.
  // The 5s budget mirrors the SC#1 contract. If the first Run already fails
  // (BUG-01 throws OR a different bug surfaces), this assertion times out
  // and the spec exits before the second-Run check.
  await expect(
    root.getByRole('tab', { name: /^Vertices \([1-9]\d*\)$/ }),
  ).toBeVisible({ timeout: 5_000 });

  const firstRunVertices = await verticesTabLabel();
  const firstRunEdges = await edgesTabLabel();
  expect(firstRunVertices).toMatch(/Vertices \([1-9]\d*\)/); // sanity

  // Sanity: no error banner after the first Run (scoped to the playground
  // root so the page-level live-region doesn't false-positive).
  await expect(playgroundAlerts).toHaveCount(0, { timeout: 1_000 });

  // ----- Second Run -----
  // Click Run a second time. Per BUG-01 hypothesis: the WASM compile
  // instance is reused across Runs; the pre-fix path stored a long-lived
  // FileHandle that wasm-bindgen destroyed on every `compileFile(handle)`
  // call (the JS wrapper invokes `handle.__destroy_into_raw()` — even
  // though Rust `FileHandle` is Copy). After the BUG-01 fix the Run path
  // uses the ad-hoc `instance.compile(source)` API which interns a per-
  // call SourceFile, so no handle is ever reused. With the bug present,
  // EITHER the vertex/edge counts revert to `(0)` (vertices reset by
  // handleRun, then never repopulate because runPipeline threw), OR the
  // `role="alert"` banner shows `null pointer passed to rust`.
  await page.getByRole('button', { name: 'Run mapping' }).click();

  // Wait for the second Run to repopulate. The component zeroes vertices
  // inside handleRun before awaiting runPipeline, so the labels may flicker
  // back to `(0)` briefly. We poll for the post-second-Run non-zero count.
  await expect(
    root.getByRole('tab', { name: /^Vertices \([1-9]\d*\)$/ }),
  ).toBeVisible({ timeout: 5_000 });

  const secondRunVertices = await verticesTabLabel();
  const secondRunEdges = await edgesTabLabel();

  // The core BUG-01 assertion: same counts both times.
  expect(secondRunVertices).toBe(firstRunVertices);
  expect(secondRunEdges).toBe(firstRunEdges);

  // No error banner after the second Run either. With BUG-01 present, this
  // is where a `null pointer passed to rust` error typically surfaces.
  await expect(playgroundAlerts).toHaveCount(0, { timeout: 1_000 });
});
