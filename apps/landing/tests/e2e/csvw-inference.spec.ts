/**
 * PLAY-09 + PLAY-11 E2E gate — automatic CSVW inference + editable preview.
 *
 * Verifies that when the playground loads a mapping with a CSV source AND
 * the CSVW descriptor panel is empty, inference runs automatically and
 * populates the editable preview within 1 s (per CONTEXT.md locked
 * trigger). The user can then edit a column type; the dirty flag suppresses
 * background re-inference; clicking "Reset to inferred" restores the
 * auto-inferred value.
 *
 * Skip-pending status (2026-05-25):
 *   - The default landing page seeds the `hello` example which DOES bundle a
 *     CSVW descriptor (hello.csvw.json). The inference-trigger requires a
 *     mapping with a CSV source AND no descriptor — that variation is
 *     scheduled for plan 09-09 (curated examples, including a
 *     hello-no-descriptor variant under `packages/examples/src/hello/variations/`).
 *   - Plan 09-10 (phase close) re-enables this spec once 09-09's curated
 *     example is wired into the landing examples dropdown.
 *
 * The unit-level guarantees (the CsvwPreview UI surface itself) are covered
 * by `packages/playground/tests/csvw-preview.test.tsx`; the inference engine
 * is covered by `packages/playground/tests/csvw-infer.test.ts` (plan 09-04).
 * What this spec adds when re-enabled is the WIRED-INTEGRATION assertion —
 * that the `<FossilPlayground/>` component's effect + the resolver + the
 * DuckDB-WASM connection actually compose into a working auto-infer flow.
 */
import { expect, test } from '@playwright/test';

test('PLAY-11: empty CSVW + valid CSV URL triggers inference within 1s', async ({
  page,
}) => {
  test.skip(
    true,
    'Pending plan 09-09: curated example variant without a CSVW descriptor + landing examples dropdown. Re-enabled by plan 09-10.',
  );

  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });
  // Wait for the editor mount gate (matches landing-run.spec.ts pattern).
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 15_000,
  });

  // Load the `hello-no-descriptor` variation via the examples dropdown that
  // plan 09-09 ships in apps/landing/. The dropdown's testid + option labels
  // are defined in 09-09's PLAN; the wiring here lands at that time.
  // Placeholder selectors (to be reconciled when 09-09 lands):
  //   await page.getByTestId('examples-dropdown').selectOption('hello-no-descriptor');

  const preview = page.getByTestId('csvw-preview');
  await expect(preview).toBeVisible({ timeout: 5_000 });
  // After auto-inference, two columns should appear (id + name from the
  // hello-no-descriptor CSV).
  await expect(page.getByTestId('csvw-col-name-0')).toHaveValue('id', {
    timeout: 2_000,
  });
  await expect(page.getByTestId('csvw-col-name-1')).toHaveValue('name');
});

test('PLAY-09: editing the inferred descriptor flows into compile (no re-inference)', async ({
  page,
}) => {
  test.skip(
    true,
    'Pending plan 09-09: curated example variant without a CSVW descriptor + landing examples dropdown. Re-enabled by plan 09-10.',
  );

  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 15_000,
  });

  const preview = page.getByTestId('csvw-preview');
  await expect(preview).toBeVisible({ timeout: 5_000 });

  // Change column 0's datatype.
  await page.getByTestId('csvw-col-type-0').selectOption('string');
  // The "(edited)" marker replaces "(auto-inferred)" — surfaces the dirty
  // state to the user.
  await expect(preview).toContainText(/edited/i);
  // The Reset button surfaces.
  await expect(page.getByTestId('csvw-reset')).toBeVisible();
  // Click Reset → the original `integer` value comes back; dirty flag clears
  // (marker returns to "(auto-inferred)").
  await page.getByTestId('csvw-reset').click();
  await expect(page.getByTestId('csvw-col-type-0')).toHaveValue('integer');
  await expect(preview).toContainText(/auto-inferred/i);
});
