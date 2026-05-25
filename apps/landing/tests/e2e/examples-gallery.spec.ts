/**
 * PLAY-05 E2E — examples dropdown is wired to EXAMPLES_MANIFEST and
 * selecting an example loads its source into the editor.
 *
 * Three cases:
 *   1. The dropdown enumerates every entry in EXAMPLES_MANIFEST (+ a
 *      "Custom" placeholder) so the count matches manifest.examples.length
 *      + 1 and each option's label matches the manifest entry.
 *   2. Selecting the `ecommerce` example surfaces its hallmark
 *      `https://example.org/ecommerce/` prefix in the editor, proving the
 *      remount-key swap propagated the new initialMapping.
 *   3. Selecting `hello-no-csvw` surfaces the
 *      `io.csv("@examples/hello-no-csvw.csv")` source declaration — the
 *      example that exercises the PLAY-11 CSVW inference path.
 *
 * If this spec fails:
 *   - Option count off: ExampleSelector misses the manifest iteration, or
 *     the "Custom" option was dropped.
 *   - Hallmark text missing: the remount-key swap didn't fire (host's
 *     handleExampleChange short-circuited, or FossilPlayground's
 *     `initialMapping` was captured stale), OR the example's mapping
 *     content drifted from the prefix we assert here.
 */
import { expect, test } from '@playwright/test';
// Import the manifest JSON directly — going through `@fossil-lang/examples`
// triggers Node ESM resolution of every per-example `index.js`, which
// imports `*.fossil?raw`. The `?raw` suffix is a Vite-only convention;
// Node throws "Missing semicolon" trying to parse a `.fossil` file as JS.
// The manifest is the single source of truth for the dropdown enumeration
// — pulling it raw side-steps the bundler-specific import gymnastics.
import EXAMPLES_MANIFEST from '../../../../packages/examples/src/manifest.json' with { type: 'json' };

test('PLAY-05: examples dropdown lists every example in the manifest', async ({
  page,
}) => {
  await page.goto('/');
  await page
    .getByTestId('example-selector')
    .waitFor({ timeout: 20_000 });

  const dropdown = page.getByTestId('example-selector-dropdown');
  // "— Custom —" placeholder + one option per manifest entry.
  const optionCount = await dropdown.locator('option').count();
  expect(optionCount).toBe(EXAMPLES_MANIFEST.examples.length + 1);

  for (const ex of EXAMPLES_MANIFEST.examples) {
    await expect(
      page.getByTestId(`example-option-${ex.id}`),
    ).toHaveText(ex.label);
  }
});

test('PLAY-05: selecting ecommerce loads its source into the editor', async ({
  page,
}) => {
  await page.goto('/');
  await page
    .getByTestId('example-selector')
    .waitFor({ timeout: 20_000 });

  // Same wasm-ready gate as landing-run.spec.ts — CodeMirror's
  // StreamParser must boot before the editor surfaces typed content.
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });

  await page
    .getByTestId('example-selector-dropdown')
    .selectOption('ecommerce');

  // The ecommerce example uses `prefix ex: <https://example.org/ecommerce/>`.
  // CodeMirror's content lives under `.cm-content` inside the editor host.
  const editor = page.locator('.cm-editor .cm-content').first();
  await expect(editor).toContainText('https://example.org/ecommerce/', {
    timeout: 5_000,
  });
});

test('PLAY-05: selecting hello-no-csvw loads its mapping (PLAY-11 inference hook)', async ({
  page,
}) => {
  await page.goto('/');
  await page
    .getByTestId('example-selector')
    .waitFor({ timeout: 20_000 });

  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });

  await page
    .getByTestId('example-selector-dropdown')
    .selectOption('hello-no-csvw');

  // The hello-no-csvw mapping's distinguishing line is the source
  // declaration pointing at `@examples/hello-no-csvw.csv`.
  const editor = page.locator('.cm-editor .cm-content').first();
  await expect(editor).toContainText(/io\.csv\(\s*"@examples\/hello-no-csvw\.csv"/, {
    timeout: 5_000,
  });
});
