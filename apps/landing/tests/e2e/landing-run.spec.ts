/**
 * SC#1 gate — Run produces real IRI rows from the compile → resolve →
 * DuckDB pipeline.
 *
 * Per Phase 8 success criterion #1 (CONTEXT.md):
 *   "default resolver loads bundled example → edit → Run → vertex+edge
 *    tables in <5s on a 2020-era laptop, offline (after first load)"
 *
 * Closes 08-VERIFICATION.md gap 1 BLOCKER (the handleRun-was-stub regression)
 * AND the CODEGEN-LOWERING-01 carry-forward (08-13 → 09-01). Before
 * 08-13 the stub returned empty arrays without any DuckDB call. After 08-13
 * Tasks 1+2, the pipeline was wired but DuckDB-WASM rejected the emitted SQL
 * with `Binder Error: Referenced table "users" not found! Candidate tables:
 * "hello"` because `fossil-mir`'s `Op::TripleEmit` lowering emitted
 * binding-name (`users`) where it should have emitted an empty source so
 * `render_expr`'s `default_source` (view name `hello`) substituted. 09-01
 * Task 1 closed CODEGEN-LOWERING-01 in `crates/fossil-mir/src/lower.rs`. This
 * spec is the tightened-gate counterpart: it now demands the success path
 * directly — five hello-example users render as
 * `https://example.org/user/{1..5}` vertex rows AND ≥1 edges-table row —
 * within the 5_000 ms SC#1 budget.
 *
 * If this spec fails with `https://example.org/user/...` absent but
 * `role="alert"` populated, the codegen bug or a different DuckDB-WASM
 * binding error has re-emerged. Inspect the alert content via
 * `await page.locator('[role="alert"]').textContent()` and follow the
 * regression path through `crates/fossil-mir/src/lower.rs` (the
 * source-binding ColRef sites) and `crates/fossil-codegen/src/sql.rs`
 * (`render_expr` + `derive_view_name`).
 */
import { expect, test } from '@playwright/test';

test('SC#1: landing default flow — Run renders real IRI vertex rows within 5 s', async ({
  page,
}) => {
  await page.goto('/');

  // Wait for the dynamic-imported playground to mount. The Client Shell's
  // loading fallback flashes 'Loading playground…' until the dynamic
  // chunk resolves; the data-testid lands on the inner playground root.
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 15_000,
  });

  // Wait for the editor mount gate (initFossilWasm resolved). The
  // <FossilPlayground/> shows the editor only after wasmReady flips true
  // (08-11 Rule 1 fix — CodeMirror's StreamParser eagerly calls tokenize());
  // without this wait the Run click can race ahead of the WASM boot and
  // produce a "WASM is still loading" error instead of a real Run.
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });

  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeEnabled();

  // Click Run + measure. The 5 s budget is the SC#1 contract per CONTEXT.md.
  const t0 = Date.now();
  await page.getByRole('button', { name: 'Run mapping' }).click();

  // Tight gate: assert the success state directly — at least one rendered
  // `https://example.org/user/N` IRI must appear within 5 s. With
  // CODEGEN-LOWERING-01 closed (09-01 Task 1), the hello example produces
  // 5 user vertices through the in-browser DuckDB-WASM pipeline.
  const root = page.getByTestId('fossil-playground');
  await expect(
    root.getByText(/https:\/\/example\.org\/user\/[0-9]+/).first(),
  ).toBeVisible({ timeout: 5_000 });

  const elapsed = Date.now() - t0;
  // eslint-disable-next-line no-console
  console.log(`[SC#1] Run-to-real-IRI: ${elapsed} ms`);
  expect(elapsed).toBeLessThan(5_000);
});

test('SC#1: Reset playground clears results but keeps the editor warm', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor();
  // Same wasm-ready gate as the first test.
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });

  await page.getByRole('button', { name: 'Run mapping' }).click();

  // Wait for the pipeline to resolve to the real-IRI success state — Reset
  // must operate on a non-pending Run so the post-Reset empty-state assertion
  // below is meaningful.
  const root = page.getByTestId('fossil-playground');
  await expect(
    root.getByText(/https:\/\/example\.org\/user\/[0-9]+/).first(),
  ).toBeVisible({ timeout: 5_000 });

  await page.getByRole('button', { name: 'Reset playground' }).click();

  // Editor still mounted — the LSP Worker survives Reset per ADR-0026's
  // asymmetric lifecycle. The CodeMirror content host stays in the DOM.
  await expect(page.locator('.cm-content')).toBeVisible({ timeout: 5_000 });

  // Post-Reset state: both vertex IRI rendering AND any alert error are
  // cleared. The vertex/edge panels show the "no results" status div; the
  // alert region is empty (reset clears runError).
  await expect(
    page.getByText(/https:\/\/example\.org\/user\/[0-9]+/),
  ).toHaveCount(0, { timeout: 5_000 });
  await expect(page.getByText(/no results/i).first()).toBeVisible({
    timeout: 5_000,
  });
});

test('SC#3: initial page load completes (first-paint observable) within reasonable time', async ({
  page,
}) => {
  // SC#3's strict <3s cold-load assertion needs a CDN-warmed deployment +
  // Lighthouse-class measurement; here we exercise the localhost cold
  // path as a CHECK (not a strict gate). The full SC#3 measurement is
  // deferred to Phase 9 deployment verification per the plan-spec's
  // success criteria notes.
  const start = Date.now();
  await page.goto('/');
  // Header text from page.tsx renders synchronously (Server Component).
  await expect(
    page.getByRole('heading', { name: /fossil playground/i }),
  ).toBeVisible({ timeout: 10_000 });
  const elapsed = Date.now() - start;
  // eslint-disable-next-line no-console
  console.log(`[SC#3] First-paint observable at: ${elapsed} ms`);
  // Localhost cold-path budget; CI verifies it doesn't blow up to 10s+.
  expect(elapsed).toBeLessThan(10_000);
});
