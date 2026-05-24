/**
 * SC#2 gate — <FossilPlayground/> mounts in a NON-Next.js host.
 *
 * Per Phase 8 success criterion #2 (CONTEXT.md):
 *   "pnpm create vite my-host --template react-ts && pnpm add
 *    @fossil-lang/playground" + <FossilPlayground resolver={...}
 *    initialMapping="..." /> in any React 18+ host renders a working
 *    playground.
 *
 * The fixture lives at apps/landing/tests/e2e/multi-host-fixture/ and is
 * served by Playwright's second webServer entry on :4173. The fixture's
 * main.tsx mounts <FossilPlayground/> against the bundled hello
 * example.
 *
 * This is the STRUCTURAL proof that the playground package is not
 * coupled to Next.js — if a future refactor adds a Next.js dependency
 * to the @fossil-lang/* graph, this spec fails because the fixture's
 * Vite build (no Next.js) won't resolve the import.
 */
import { expect, test } from '@playwright/test';

const FIXTURE_URL = `http://localhost:${process.env.FIXTURE_PORT ?? 4173}`;

test('SC#2: <FossilPlayground/> mounts in a non-Next.js Vite host', async ({
  page,
}) => {
  await page.goto(FIXTURE_URL);

  // Header from the fixture's index.html — proves we hit the right server
  // (not accidentally the Next.js landing on :3000).
  await expect(
    page.getByRole('heading', { name: /multi-host fixture/i }),
  ).toBeVisible();

  // Component test id present + Run button enabled — the playground
  // composed itself successfully in a non-Next.js host.
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 15_000,
  });
  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeEnabled();

  // Editor mounted (CodeMirror's content host renders into the editor
  // section). Proves the LSP Worker boot path + CodeMirror init both
  // work in a Vite host.
  await expect(page.locator('.cm-content')).toBeVisible({ timeout: 10_000 });

  // Sanity: no React error boundary tripped, no "module not found"-class
  // crash in the console.
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  // Give any deferred boot errors a tick to surface.
  await page.waitForTimeout(500);
  expect(errors).toEqual([]);
});

test('SC#2: Run button fires in the non-Next.js host (smoke)', async ({
  page,
}) => {
  await page.goto(FIXTURE_URL);
  await page.getByTestId('fossil-playground').waitFor({ timeout: 15_000 });

  // We're not asserting the timing budget here (that's SC#1's job on the
  // production Next.js host); just that the button click doesn't throw
  // and the result region populates with the expected captions.
  await page.getByRole('button', { name: 'Run mapping' }).click();

  await expect(
    page.getByText(/vertices/i, { exact: false }).first(),
  ).toBeVisible({ timeout: 5_000 });
  await expect(
    page.getByText(/edges/i, { exact: false }).first(),
  ).toBeVisible({ timeout: 5_000 });
});
