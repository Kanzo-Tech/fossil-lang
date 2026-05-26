/**
 * EDIT-01 + EDIT-02 + EDIT-03 oracle — Phase 11 plan 11-04.
 *
 * Proves that @fossil-lang/editor (1) renders standalone in a vanilla
 * Vite + React host (NO Next.js / NO Tailwind / NO shadcn — EDIT-01),
 * (2) accepts the three locked transports identically (EDIT-02), and
 * (3) UX (rendering anatomy + interactivity + @-autocomplete) is
 * identical across all three transport modes (EDIT-03).
 *
 * Two-page model:
 *   - /editor.html — Worker + HTTP editors (co-mounted)
 *   - /editor-null.html — Null editor (isolated; non-firing assertion
 *     is structurally definitive)
 *
 * Both pages served by the existing vite-preview webServer on :4173.
 *
 * HTTP transport is mocked via Playwright route fulfill — CONTEXT.md
 * Claude's-discretion guidance prefers route fulfill over MSW.
 */
import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

const FIXTURE_PORT = Number(process.env.FIXTURE_PORT ?? 4173);
const EDITOR_URL = `http://localhost:${FIXTURE_PORT}/editor.html`;
const EDITOR_NULL_URL = `http://localhost:${FIXTURE_PORT}/editor-null.html`;

// Canned JSON-RPC hover response. The HttpTransport editor will send
// initialize + textDocument/didOpen + textDocument/hover (etc); the route
// handler dispatches a reasonable response for whatever method arrives.
const HOVER_RESPONSE = {
  jsonrpc: '2.0',
  id: 1,
  result: {
    contents: {
      kind: 'markdown',
      value: '**Mocked hover** from Playwright route fulfill.',
    },
  },
};

test.describe('EDIT-01/02/03 — @fossil-lang/editor multi-transport oracle', () => {
  test('Worker + HTTP cells render on /editor.html (EDIT-01 + EDIT-02 modes #1+#2)', async ({
    page,
  }) => {
    await page.route('**/api/fossil/analyze', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(HOVER_RESPONSE),
      });
    });
    await page.goto(EDITOR_URL);
    await page.waitForSelector('[data-testid="editor-root"]', {
      timeout: 10_000,
    });
    await expect(
      page.locator('[data-testid="editor-cell-worker"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="editor-cell-http"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="editor-cell-worker"] .cm-content'),
    ).toHaveCount(1);
    await expect(
      page.locator('[data-testid="editor-cell-http"] .cm-content'),
    ).toHaveCount(1);
  });

  test('Null cell renders on /editor-null.html (EDIT-02 mode #3)', async ({
    page,
  }) => {
    await page.goto(EDITOR_NULL_URL);
    await page.waitForSelector('[data-testid="editor-root"]', {
      timeout: 10_000,
    });
    await expect(
      page.locator('[data-testid="editor-cell-null"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="editor-cell-null"] .cm-content'),
    ).toHaveCount(1);
  });

  test('initial source text appears in editors across both pages (EDIT-03 paridad)', async ({
    page,
  }) => {
    const probe = 'Phase 11 EDIT oracle';

    // Worker + HTTP page
    await page.route('**/api/fossil/analyze', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(HOVER_RESPONSE),
      });
    });
    await page.goto(EDITOR_URL);
    await page.waitForSelector('[data-testid="editor-root"]', {
      timeout: 10_000,
    });
    await expect(
      page.locator('[data-testid="editor-cell-worker"] .cm-content'),
    ).toContainText(probe);
    await expect(
      page.locator('[data-testid="editor-cell-http"] .cm-content'),
    ).toContainText(probe);

    // Null page
    await page.goto(EDITOR_NULL_URL);
    await page.waitForSelector('[data-testid="editor-root"]', {
      timeout: 10_000,
    });
    await expect(
      page.locator('[data-testid="editor-cell-null"] .cm-content'),
    ).toContainText(probe);
  });

  test('HttpTransport sends at least one POST to /api/fossil/analyze (EDIT-02 mode #2)', async ({
    page,
  }) => {
    let routeHitCount = 0;
    await page.route('**/api/fossil/analyze', async (route) => {
      routeHitCount += 1;
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(HOVER_RESPONSE),
      });
    });
    await page.goto(EDITOR_URL);
    await page.waitForSelector('[data-testid="editor-cell-http"]', {
      timeout: 10_000,
    });
    // Click into the HTTP editor's doc to trigger a textDocument/* request.
    await page
      .locator('[data-testid="editor-cell-http"] .cm-content')
      .click();
    // Wait a beat for the LSP client to send.
    await page.waitForTimeout(500);
    expect(routeHitCount).toBeGreaterThanOrEqual(1);
  });

  test('NullTransport on isolated page sends ZERO requests to /api/fossil/analyze (EDIT-02 mode #3 — structurally definitive)', async ({
    page,
  }) => {
    let routeHitCount = 0;
    await page.route('**/api/fossil/analyze', async (route) => {
      routeHitCount += 1;
      await route.continue();
    });
    await page.goto(EDITOR_NULL_URL);
    await page.waitForSelector('[data-testid="editor-cell-null"]', {
      timeout: 10_000,
    });
    await page
      .locator('[data-testid="editor-cell-null"] .cm-content')
      .click();
    await page.waitForTimeout(500);
    // ISOLATED page — no HttpTransport editor co-mounted. The counter
    // MUST be exactly zero. Any value > 0 is a regression.
    expect(routeHitCount).toBe(0);
  });

  test('@-prefix autocomplete works against ConnectionResolver (EDIT-03 SC#3)', async ({
    page,
  }) => {
    await page.route('**/api/fossil/analyze', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(HOVER_RESPONSE),
      });
    });
    await page.goto(EDITOR_URL);
    await page.waitForSelector('[data-testid="editor-cell-worker"]', {
      timeout: 10_000,
    });
    // Focus the Worker editor's content (CodeMirror's contenteditable host).
    const cmContent = page.locator(
      '[data-testid="editor-cell-worker"] .cm-content',
    );
    await cmContent.click();
    // Move cursor to end of doc and type the @ trigger.
    await page.keyboard.press('Control+End');
    await page.keyboard.type('@');
    // CM6's autocomplete opens with a small async delay (extension
    // schedules via requestIdleCallback / setTimeout). 500ms is a safe
    // upper bound on dev machines + CI.
    await page.waitForTimeout(500);
    // CM6 renders the autocomplete tooltip as .cm-tooltip-autocomplete.
    // The resolver provides ≥ 2 connectors via buildResolverExamples()
    // so the popup MUST be populated with options.
    const popup = page.locator('.cm-tooltip-autocomplete').first();
    await expect(popup).toBeVisible({ timeout: 5_000 });
    // At least one option visible in the popup list.
    const options = popup.locator('li, [role="option"]');
    const optionCount = await options.count();
    expect(optionCount).toBeGreaterThanOrEqual(1);
  });

  test('kanzoTheme CSS variables reach the editor surface (EDIT-01 SC#1)', async ({
    page,
  }) => {
    await page.route('**/api/fossil/analyze', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(HOVER_RESPONSE),
      });
    });
    await page.goto(EDITOR_URL);
    await page.waitForSelector('[data-testid="editor-root"]', {
      timeout: 10_000,
    });
    const root = page.locator('[data-testid="editor-root"]');
    const styleAttr = await root.getAttribute('style');
    // KanzoThemeProvider applies --fossil-* vars inline on its wrapping div.
    expect(styleAttr || '').toMatch(/--fossil-/);
    // The editor's border-color computes from --fossil-colors-border.
    const fossilEditor = page.locator(
      '[data-testid="editor-cell-worker"] .fossil-editor',
    );
    const computedBorder = await fossilEditor.evaluate(
      (el) => getComputedStyle(el).borderColor,
    );
    // Reject the system default 'rgb(0, 0, 0)' (or 'rgba(0, 0, 0, 0)') —
    // border must resolve to a kanzoTheme palette colour.
    expect(computedBorder).not.toBe('rgb(0, 0, 0)');
    expect(computedBorder).not.toBe('rgba(0, 0, 0, 0)');
  });

  test('axe-core finds zero WCAG 2.1 A + AA violations on both editor oracles', async ({
    page,
  }) => {
    await page.route('**/api/fossil/analyze', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(HOVER_RESPONSE),
      });
    });
    // Worker + HTTP page
    await page.goto(EDITOR_URL);
    await page.waitForSelector('[data-testid="editor-root"]', {
      timeout: 10_000,
    });
    let results = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .exclude('.cm-content')
      .exclude('[data-radix-scroll-area-viewport]')
      .analyze();
    if (results.violations.length > 0) {
      // eslint-disable-next-line no-console
      console.log(
        '[EDIT-01/02/03 worker+http] axe violations:',
        JSON.stringify(results.violations, null, 2),
      );
    }
    expect(results.violations).toEqual([]);

    // Null page
    await page.goto(EDITOR_NULL_URL);
    await page.waitForSelector('[data-testid="editor-root"]', {
      timeout: 10_000,
    });
    results = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .exclude('.cm-content')
      .exclude('[data-radix-scroll-area-viewport]')
      .analyze();
    if (results.violations.length > 0) {
      // eslint-disable-next-line no-console
      console.log(
        '[EDIT-01/02/03 null] axe violations:',
        JSON.stringify(results.violations, null, 2),
      );
    }
    expect(results.violations).toEqual([]);
  });
});
