/**
 * PLAY-04 E2E — URL-fragment permalink survives reload.
 *
 * The single hardest invariant of PLAY-04: a user types in the editor,
 * the URL fragment updates (debounced ~200ms inside the component +
 * mirrored via host.replaceState), and a full page reload restores the
 * same editor content from `window.location.hash`.
 *
 * This is the "paper-permanence" check from CONTEXT.md — a reviewer
 * clicking a 2026 permalink in 2027 should see the same playground
 * state. We don't simulate 2027 here; we just prove the encode →
 * fragment → decode → render loop closes within a single browser
 * session, which is the strongest claim the suite can make today.
 *
 * Why a marker comment (not the full mapping body): the hello example
 * is bundled at module load time, so a fresh page WITH an empty
 * fragment ALSO shows the hello content. We need a uniquely-typed
 * suffix that only the permalink could have restored.
 *
 * If this spec fails the regression vector is one of:
 *   - usePermalink's debounced encode never fires (timer ref leak)
 *   - PlaygroundHost's replaceState short-circuit returns false
 *     positives (the no-op guard)
 *   - usePermalink's decode latch double-fires on the post-reload
 *     mount and clobbers the decoded source (hydratedRef regression)
 *   - the encoded permalink exceeds MAX_PERMALINK_BYTES — bumping
 *     the cap or implementing a Gist fallback would be the followup.
 */
import { expect, test } from '@playwright/test';

test('PLAY-04: editing the editor updates the URL fragment, reload restores state', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 20_000,
  });

  // Same wasm-ready gate as the other landing specs — CodeMirror's
  // StreamParser must boot before we can type into the editor.
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });

  // On a fresh load the component's debounced onStateChange fires once
  // with the initial helloExample-derived state — the host writes that
  // into the fragment via replaceState. We capture this baseline hash so
  // we can detect when our typed-marker edit actually changes it (the
  // post-debounce fragment MUST differ from the baseline).
  await page.waitForFunction(
    () => window.location.hash.length > 1,
    null,
    { timeout: 3_000 },
  );
  const baselineHash = await page.evaluate(() => window.location.hash);
  expect(baselineHash).toMatch(/^#[A-Za-z0-9_-]+$/);

  // Type a uniquely-identifiable marker into the editor. Date.now() in
  // the marker makes it cross-session-unique so a stale fragment from a
  // previous test run can't false-positive.
  const editor = page.locator('.cm-content').first();
  await editor.click();
  const marker = `# PLAY-04-permalink-marker-${Date.now()}`;
  await page.keyboard.press('End');
  await page.keyboard.press('Enter');
  await page.keyboard.type(marker);

  // Wait until the URL fragment moves OFF the baseline — that's the
  // observable signal that the debounced encode picked up our edit and
  // the host wrote a fresh hash. 3000ms gives plenty of headroom for a
  // slow CI runner (debounce is 200ms internally).
  await page.waitForFunction(
    (baseline) =>
      window.location.hash.length > 1 && window.location.hash !== baseline,
    baselineHash,
    { timeout: 3_000 },
  );
  const hashAfterEdit = await page.evaluate(() => window.location.hash);
  expect(hashAfterEdit).toMatch(/^#[A-Za-z0-9_-]+$/);
  expect(hashAfterEdit).not.toBe(baselineHash);

  // Reload — the browser preserves window.location.hash across reload by
  // protocol, so on mount the host reads it back and hydrates the
  // component via initialPermalink. The marker we typed must be present
  // in the restored editor.
  await page.reload();
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 20_000,
  });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });

  // After reload, the fragment is still a valid permalink (the host wrote
  // a fresh one via replaceState after hydration emitted onStateChange,
  // and gzip determinism MAY make it byte-identical to hashAfterEdit but
  // we don't insist on byte equality — what matters is that the URL is
  // still a non-empty, well-formed permalink AND the editor content was
  // restored from it).
  const hashAfterReload = await page.evaluate(() => window.location.hash);
  expect(hashAfterReload).toMatch(/^#[A-Za-z0-9_-]+$/);

  // The marker comment must appear in the editor's restored content.
  // CodeMirror lazily virtualizes long documents; .cm-content's
  // textContent gives the visible portion which is sufficient for the
  // hello example (small enough to fit in one viewport).
  const restored = (await page
    .locator('.cm-content')
    .first()
    .textContent({ timeout: 5_000 })) ?? '';
  expect(restored).toContain(marker);
});
