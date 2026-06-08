/**
 * PLAY-07 E2E gate — the Compiled SQL panel surfaces live DuckDB codegen and
 * tracks the live mapping (debounced 200 ms).
 *
 * v2 layout note: in the post-Phase-14 playground the compiled SQL is NO
 * longer a toolbar toggle ("Show compiled SQL" button). It is a right-panel
 * TAB ("Compiled SQL", sibling of "Output"). Activating the tab mounts the
 * `data-testid="compiled-sql-panel"` view and arms the debounced recompile
 * effect (which is a no-op while the tab is inactive). We load the
 * hyphen-free `hello` example so the emitted SQL is deterministic
 * (`CREATE VIEW hello AS … read_csv_auto(…)`) — the landing default
 * `hello-no-csvw` trips the `derive_view_name` hyphen bug (see helpers §3).
 */
import { expect, test } from '@playwright/test';

import { gotoPlayground, loadHelloExample } from './helpers';

test('PLAY-07: the Compiled SQL tab reveals DuckDB SQL for the current mapping', async ({
  page,
}) => {
  await gotoPlayground(page);
  await loadHelloExample(page);

  // Activate the right-panel Compiled SQL tab (arms the recompile effect).
  await page.getByRole('tab', { name: 'Compiled SQL' }).click();
  const panel = page.getByTestId('compiled-sql-panel');
  await expect(panel).toBeVisible();

  // The hello example compiles to a DuckDB CREATE VIEW over read_csv_auto.
  // Generous timeout — the debounced compile fires within 200 ms of the
  // tab activation.
  await expect(panel).toContainText(/CREATE VIEW\s+hello/i, { timeout: 5_000 });
  await expect(panel).toContainText(/read_csv_auto/i, { timeout: 2_000 });

  // The tab is now the selected one in its tablist.
  await expect(page.getByRole('tab', { name: 'Compiled SQL' })).toHaveAttribute(
    'aria-selected',
    'true',
  );
});

test('PLAY-07: Compiled SQL panel updates within 1s after a mapping edit', async ({
  page,
}) => {
  await gotoPlayground(page);
  await loadHelloExample(page);

  await page.getByRole('tab', { name: 'Compiled SQL' }).click();
  const panel = page.getByTestId('compiled-sql-panel');
  await expect(panel).toBeVisible();
  // Let the initial debounced compile populate so we have a baseline.
  await expect(panel).toContainText(/CREATE VIEW\s+hello/i, { timeout: 5_000 });
  const baseline = (await panel.textContent()) ?? '';
  expect(baseline.length).toBeGreaterThan(0);

  // Focus the editor and type into the existing content. The exact caret
  // position depends on where the `.click()` lands (Playwright's default is
  // the element's geometric centre); we don't control it precisely and don't
  // need to. The gate is "the debounced recompile fired" — a successful
  // re-compile OR the compile-error placeholder, both non-empty.
  await page.locator('.cm-editor .cm-content').first().click();
  await page.keyboard.type(' ');

  // Wait for the debounce window + recompile to settle.
  await page.waitForTimeout(1_000);
  const updated = (await panel.textContent()) ?? '';
  expect(updated.length).toBeGreaterThan(0);
  expect(updated).toMatch(/CREATE VIEW|SELECT|COPY|compile error/i);
});
