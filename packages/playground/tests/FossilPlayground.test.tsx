/**
 * Mount-smoke + handleRun-wiring tests for <FossilPlayground/>.
 *
 * Covers:
 *   - The component mounts without throwing against a mock resolver
 *   - The Run + Reset buttons are present + reachable by a11y queries
 *   - The CodeMirror editor host div is in the DOM
 *   - initialMapping is respected (the editor host is constructed)
 *   - Clicking Run invokes runPipeline with the expected deps + input shape
 *     (Task 2: 08-13 — the BLOCKER stub is gone; handleRun delegates)
 *
 * Real LSP/DuckDB Worker boot is mocked in tests/setup.ts. Network-running
 * Run paths are exercised in apps/landing/ via Playwright (08-11 + 08-13
 * Task 3).
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor, act, fireEvent } from '@testing-library/react';
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

/**
 * Task 2 (08-13) — assert handleRun now CALLS runPipeline (the KNOWN GAP
 * stub is gone). Mock the runPipeline module so the test doesn't depend on
 * the real WASM/DuckDB boot.
 */
vi.mock('../src/run/runPipeline.js', async (importActual) => {
  // Preserve the original type exports / runPipeline shape; override the
  // function with a mock.
  const actual = await importActual<typeof import('../src/run/runPipeline.js')>();
  return {
    ...actual,
    runPipeline: vi.fn(),
  };
});

describe('<FossilPlayground/> handleRun wiring (08-13 Task 2)', () => {
  it('clicking Run invokes runPipeline with the expected deps + input', async () => {
    const { runPipeline } = await import('../src/run/runPipeline.js');
    const mockRun = vi.mocked(runPipeline);
    mockRun.mockResolvedValue({
      vertices: [{ id: 'https://example.org/user/42', name: 'Alice' }],
      edges: [
        {
          source: 'https://example.org/user/42',
          target: 'Alice',
          predicate: 'https://example.org/name',
        },
      ],
    });

    const onRun = vi.fn();
    render(
      <FossilPlayground
        resolver={mockResolver}
        wasmUrl="https://mock/fossil.wasm"
        initialMapping="prefix ex: <https://example.org/>"
        onRun={onRun}
      />,
    );

    // Wait for wasmReady (the mocked initFossilWasm resolves immediately).
    await waitFor(() => {
      expect(document.querySelector('.fossil-editor')).toBeTruthy();
    });

    const runBtn = screen.getByRole('button', { name: /run mapping/i });
    await act(async () => {
      fireEvent.click(runBtn);
    });

    // Assertion 1: runPipeline was called.
    expect(mockRun).toHaveBeenCalledTimes(1);

    // Assertion 2: the deps shape contains compile + getDuckDb.
    const [deps, input] = mockRun.mock.calls[0]!;
    expect(typeof deps.compile).toBe('function');
    expect(typeof deps.getDuckDb).toBe('function');

    // Assertion 3: the input carries resolver + mapping + the default cap.
    expect(input.resolver).toBe(mockResolver);
    expect(input.mapping).toMatch(/^prefix ex:/);
    expect(input.maxResolvedBytes).toBe(10 * 1024 * 1024);

    // Assertion 4: the result rows landed in component state (the vertex
    // id renders into the tabular fallback row). The same IRI appears in
    // both the Vertices table (as the `id` cell) AND the Edges table (as
    // the `source` cell), so getAllByText surfaces multiple matches.
    await waitFor(() => {
      const matches = screen.getAllByText('https://example.org/user/42');
      expect(matches.length).toBeGreaterThan(0);
    });

    // Assertion 5: the onRun prop fired with the same result shape.
    expect(onRun).toHaveBeenCalledWith({
      vertices: expect.arrayContaining([
        expect.objectContaining({ id: 'https://example.org/user/42' }),
      ]),
      edges: expect.arrayContaining([
        expect.objectContaining({
          source: 'https://example.org/user/42',
          target: 'Alice',
        }),
      ]),
    });
  });

  it('runPipeline errors surface as role="alert" + onError', async () => {
    const { runPipeline } = await import('../src/run/runPipeline.js');
    const mockRun = vi.mocked(runPipeline);
    mockRun.mockRejectedValue(new Error('mock pipeline failure'));

    const onError = vi.fn();
    render(
      <FossilPlayground
        resolver={mockResolver}
        wasmUrl="https://mock/fossil.wasm"
        onError={onError}
      />,
    );
    await waitFor(() => {
      expect(document.querySelector('.fossil-editor')).toBeTruthy();
    });

    const runBtn = screen.getByRole('button', { name: /run mapping/i });
    await act(async () => {
      fireEvent.click(runBtn);
    });

    await waitFor(() => {
      const alert = screen.queryByRole('alert');
      expect(alert?.textContent).toMatch(/mock pipeline failure/);
    });
    expect(onError).toHaveBeenCalledWith(expect.any(Error));
  });
});
