/**
 * PLAY-09 + PLAY-11 E2E gate — automatic CSVW inference + editable preview.
 *
 * Verifies that when the playground loads a mapping with a CSV source AND
 * the CSVW descriptor panel is empty, inference runs automatically and
 * populates the editable preview within ~1 s (per 09-CONTEXT.md locked
 * trigger). The user can then edit a column type; the dirty flag suppresses
 * background re-inference; clicking "Reset to inferred" restores the
 * auto-inferred value.
 *
 * Re-enabled by plan 09-10 (Phase 9 close) — plan 09-09 shipped the
 * `hello-no-csvw` curated example and wired it into the landing examples
 * dropdown (ExampleSelector), unblocking this spec.
 *
 * Loading strategy: rather than rebuilding a permalink encoder in the spec
 * (which would couple the test to internal package paths), we use the same
 * production code path the user does — pick `hello-no-csvw` from the
 * dropdown that 09-09 wired into PlaygroundHost. This proves the integrated
 * UX (dropdown → remount-key swap → empty-CSVW → auto-infer → preview).
 *
 * The unit-level guarantees (the CsvwPreview UI surface itself) are covered
 * by `packages/playground/tests/csvw-preview.test.tsx`; the inference engine
 * is covered by `packages/playground/tests/csvw-infer.test.ts` (plan 09-04).
 * What this spec adds is the WIRED-INTEGRATION assertion — that the
 * `<FossilPlayground/>` component's effect + the resolver + the DuckDB-WASM
 * connection actually compose into a working auto-infer flow.
 */
import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.goto('/');
  // ExampleSelector mounts as soon as the host's client shell hydrates; the
  // editor mount gate matches landing-run.spec.ts pattern.
  await page
    .getByTestId('example-selector')
    .waitFor({ timeout: 20_000 });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 15_000,
  });
  // Switch to the hello-no-csvw example via the dropdown 09-09 wired in.
  // PlaygroundHost.handleExampleChange triggers the remount-key swap, which
  // re-seeds <FossilPlayground/> with empty csvw — the precondition the
  // PLAY-11 auto-infer effect watches for.
  await page
    .getByTestId('example-selector-dropdown')
    .selectOption('hello-no-csvw');
  // Editor content swap is the cheapest "remount took effect" signal —
  // matches the same assertion examples-gallery.spec.ts uses.
  await expect(page.locator('.cm-editor .cm-content').first()).toContainText(
    /io\.csv\(\s*"@examples\/hello-no-csvw\.csv"/,
    { timeout: 5_000 },
  );
});

test('PLAY-11: empty CSVW + valid CSV URL triggers inference', async ({
  page,
}) => {
  const preview = page.getByTestId('csvw-preview');
  // Auto-infer effect fires on the empty-csvw + io.csv literal combo. The
  // upper bound on infer time is the DuckDB-WASM cold-boot for the inner
  // transient connection + read_csv_auto descriptor (~1 s steady-state per
  // 09-CONTEXT.md; allow generous CI headroom).
  await expect(preview).toBeVisible({ timeout: 10_000 });

  // After auto-inference, two columns should appear (id + name from the
  // hello-no-csvw CSV — the same schema as hello.csv with the descriptor
  // sidecar absent).
  await expect(page.getByTestId('csvw-col-name-0')).toHaveValue('id', {
    timeout: 5_000,
  });
  await expect(page.getByTestId('csvw-col-name-1')).toHaveValue('name');
});

test('PLAY-09: editing the inferred descriptor surfaces dirty + reset affordance', async ({
  page,
}) => {
  const preview = page.getByTestId('csvw-preview');
  await expect(preview).toBeVisible({ timeout: 10_000 });
  // Inference must have populated before we attempt to edit; otherwise the
  // <select> options array would be empty and selectOption('string') would
  // silently no-op.
  await expect(page.getByTestId('csvw-col-type-0')).toBeVisible({
    timeout: 5_000,
  });

  // Change column 0's datatype. The CsvwPreview component flips its dirty
  // marker text + reveals the Reset button.
  await page.getByTestId('csvw-col-type-0').selectOption('string');
  // The "(edited)" marker replaces "(auto-inferred)" — surfaces the dirty
  // state to the user.
  await expect(preview).toContainText(/edited/i, { timeout: 2_000 });
  // The Reset button surfaces.
  await expect(page.getByTestId('csvw-reset')).toBeVisible();
  // Click Reset → the original `integer` value comes back; dirty flag
  // clears (marker returns to "(auto-inferred)").
  await page.getByTestId('csvw-reset').click();
  await expect(page.getByTestId('csvw-col-type-0')).toHaveValue('integer', {
    timeout: 2_000,
  });
  await expect(preview).toContainText(/auto-inferred/i);
});
