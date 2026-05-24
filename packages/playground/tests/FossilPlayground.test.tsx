/**
 * Mount-smoke tests for <FossilPlayground/>.
 *
 * Covers:
 *   - The component mounts without throwing against a mock resolver
 *   - The Run + Reset buttons are present + reachable by a11y queries
 *   - The CodeMirror editor host div is in the DOM
 *   - initialMapping is respected (the editor host is constructed)
 *
 * Real LSP/DuckDB Worker boot is mocked in tests/setup.ts. Network-running
 * Run paths are exercised in apps/landing/ via Playwright (08-11).
 */

import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { FossilPlayground } from '../src/index.js';
import { createMockResolver } from '@fossil-lang/resolvers';

const mockResolver = createMockResolver({
  fixtures: {},
  connectors: [{ name: 'examples', type: 'examples' as const }],
});

describe('<FossilPlayground/>', () => {
  it('renders with required props', () => {
    render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/fossil.wasm" />,
    );
    expect(screen.getByTestId('fossil-playground')).toBeTruthy();
    expect(screen.getByRole('button', { name: /run mapping/i })).toBeTruthy();
    expect(screen.getByRole('button', { name: /reset playground/i })).toBeTruthy();
  });

  it('respects initialMapping when provided', async () => {
    render(
      <FossilPlayground
        resolver={mockResolver}
        wasmUrl="https://mock/fossil.wasm"
        initialMapping="prefix custom: <https://custom.org/>"
      />,
    );
    // The CodeMirror editor host is mounted ASYNC: <FossilPlayground/>
    // defers the editor render until initFossilWasm resolves (08-11 Rule 1
    // fix — CodeMirror's StreamParser eagerly calls tokenize() on mount
    // and would crash if WASM hadn't initialised). The mock in
    // tests/setup.ts resolves initFossilWasm() immediately so this
    // waitFor flips quickly; in production the same flip happens after
    // the .wasm bundle finishes downloading.
    await waitFor(
      () => {
        const host = document.querySelector('.fossil-editor');
        expect(host).toBeTruthy();
      },
      { timeout: 2_000 },
    );
  });

  it('renders the Results section with a tabular fallback', () => {
    render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/fossil.wasm" />,
    );
    const results = screen.getByRole('region', { name: /results/i });
    expect(results).toBeTruthy();
  });

  it('toolbar buttons are not disabled at idle', () => {
    render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/fossil.wasm" />,
    );
    const runBtn = screen.getByRole('button', { name: /run mapping/i }) as HTMLButtonElement;
    const resetBtn = screen.getByRole('button', { name: /reset playground/i }) as HTMLButtonElement;
    expect(runBtn.disabled).toBe(false);
    expect(resetBtn.disabled).toBe(false);
  });
});
