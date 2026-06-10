/**
 * PLAY-04 E2E — URL-fragment permalink survives reload.
 *
 * The single hardest invariant of PLAY-04: a user types in the editor, the
 * URL fragment updates (debounced ~200 ms inside the component + mirrored via
 * host.replaceState), and a full page reload restores the same editor content
 * from `window.location.hash`.
 *
 * v2 note: the initial navigation MUST settle past the OFFLINE-01 Service
 * Worker's controlled reload before we type — otherwise the reload lands
 * mid-edit and wipes the typed marker (the original symptom: the restored
 * editor showed the default `hello-no-csvw` example, not the marker). Both
 * navigations therefore go through the networkidle/editor-ready gate
 * (gotoPlayground + an explicit post-reload settle).
 *
 * Regression vectors if this fails: usePermalink's debounced encode never
 * fires; PlaygroundHost's replaceState no-op guard false-positives;
 * usePermalink's decode latch double-fires and clobbers the decoded source;
 * or the encoded permalink exceeds MAX_PERMALINK_BYTES.
 */
import { expect, test } from '@playwright/test';

import { gotoPlayground, waitForReady } from './helpers';

test('PLAY-04: editing the editor updates the URL fragment, reload restores state', async ({
  page,
}) => {
  await gotoPlayground(page);

  // On a fresh load the component's debounced onStateChange fires once with
  // the initial example-derived state — the host writes that into the
  // fragment. Capture this baseline hash so we can detect when our typed
  // marker edit actually changes it.
  await page.waitForFunction(() => window.location.hash.length > 1, null, {
    timeout: 3_000,
  });
  const baselineHash = await page.evaluate(() => window.location.hash);
  expect(baselineHash).toMatch(/^#[A-Za-z0-9_-]+$/);

  // Append a uniquely-identifiable marker at the END of the document.
  // Date.now() makes it cross-session-unique so a stale fragment can't
  // false-positive. The marker is a space-free `//` fossil line comment:
  //   - `//` (not `#`) is fossil's comment syntax, so it's lexically inert.
  //   - no interior spaces — `.cm-content`.textContent() concatenates line
  //     spans and the editor normalises a leading "# " oddly (an earlier
  //     `# PLAY-04…` marker round-tripped but lost its post-`#` space).
  // Select-all then collapse-right lands the caret at the true document end
  // (more robust than `End`, which only reaches the clicked line's end).
  const editor = page.locator('.cm-content').first();
  await editor.click();
  const marker = `//PLAY-04-permalink-marker-${Date.now()}`;
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Enter');
  await page.keyboard.type(marker);

  // Wait until the URL fragment moves OFF the baseline — the observable
  // signal that the debounced encode picked up our edit.
  await page.waitForFunction(
    (baseline) =>
      window.location.hash.length > 1 && window.location.hash !== baseline,
    baselineHash,
    { timeout: 3_000 },
  );
  const hashAfterEdit = await page.evaluate(() => window.location.hash);
  expect(hashAfterEdit).toMatch(/^#[A-Za-z0-9_-]+$/);
  expect(hashAfterEdit).not.toBe(baselineHash);

  // Reload — the browser preserves window.location.hash by protocol, so on
  // mount the host reads it back and hydrates via initialPermalink. Settle
  // past the SW reload + editor-mount gate again before reading content.
  await page.reload();
  await page.waitForLoadState('networkidle');
  await waitForReady(page);

  const hashAfterReload = await page.evaluate(() => window.location.hash);
  expect(hashAfterReload).toMatch(/^#[A-Za-z0-9_-]+$/);

  // The marker comment must appear in the editor's restored content.
  const restored =
    (await page
      .locator('.cm-content')
      .first()
      .textContent({ timeout: 5_000 })) ?? '';
  expect(restored).toContain(marker);
});
