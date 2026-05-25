/**
 * CompiledSqlPanel — Vitest component tests (PLAY-07).
 *
 * Asserts the read-only CodeMirror panel:
 *   1. Renders with role="region" + accessible name "Compiled DuckDB SQL"
 *      (a11y contract — A11Y-01 gate downstream expects the landmark).
 *   2. Renders the supplied SQL content (CodeMirror's content host carries
 *      the doc text — checked via the testid wrapper's textContent).
 *   3. Updating the sql prop REPLACES (does not append to) the doc — the
 *      no-op-guard branch from RESEARCH.md Pitfall: "avoid setText every
 *      keystroke" still triggers a real replace when the content differs.
 *
 * The CodeMirror DOM is rendered via a real EditorView (no mock) — the
 * happy-dom setup file at `tests/setup.ts` supplies the DOM globals; the
 * EditorView only needs `document` + `requestAnimationFrame` to mount.
 */

import { afterEach, describe, test, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { CompiledSqlPanel } from '../src/compiled-sql/index.js';

afterEach(() => {
  cleanup();
});

describe('CompiledSqlPanel (PLAY-07)', () => {
  test('renders with role="region" + accessible name "Compiled DuckDB SQL"', () => {
    render(<CompiledSqlPanel sql="" />);
    const region = screen.getByRole('region', {
      name: /compiled duckdb sql/i,
    });
    expect(region).toBeTruthy();
    // testid for the E2E spec downstream
    expect(screen.getByTestId('compiled-sql-panel')).toBeTruthy();
  });

  test('renders the provided SQL text in the CodeMirror content host', () => {
    const sample =
      "CREATE OR REPLACE TABLE hello AS SELECT * FROM read_csv_auto('h.csv');";
    render(<CompiledSqlPanel sql={sample} />);
    const region = screen.getByTestId('compiled-sql-panel');
    // CodeMirror renders the doc as DOM lines under .cm-content; assert
    // the readable text contains the expected fragments. Whitespace inside
    // CodeMirror's per-line spans is collapsed at the textContent level —
    // assert non-overlapping substrings instead of the whole string.
    const text = region.textContent ?? '';
    expect(text).toContain('CREATE OR REPLACE TABLE');
    expect(text).toContain('read_csv_auto');
  });

  test('updating sql prop replaces the doc content (no append)', async () => {
    const { rerender } = render(<CompiledSqlPanel sql="SELECT 1;" />);
    const region = screen.getByTestId('compiled-sql-panel');
    expect(region.textContent ?? '').toContain('SELECT 1');
    rerender(<CompiledSqlPanel sql="SELECT 2;" />);
    // Allow a microtask for CodeMirror dispatch to settle into the DOM.
    await new Promise((resolve) => setTimeout(resolve, 10));
    const updated = region.textContent ?? '';
    expect(updated).not.toContain('SELECT 1');
    expect(updated).toContain('SELECT 2');
  });

  test('theme prop drives the data-theme attribute', () => {
    const { rerender } = render(<CompiledSqlPanel sql="" theme="light" />);
    const region = screen.getByTestId('compiled-sql-panel');
    expect(region.getAttribute('data-theme')).toBe('light');
    rerender(<CompiledSqlPanel sql="" theme="dark" />);
    expect(region.getAttribute('data-theme')).toBe('dark');
  });
});
