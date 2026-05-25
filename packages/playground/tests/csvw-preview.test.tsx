/**
 * CsvwPreview — Vitest component tests (PLAY-09 + PLAY-11).
 *
 * Scope:
 *   - Empty-state placeholder when value is empty.
 *   - One row per column with name input + datatype <select>.
 *   - Editing a datatype calls onChange with a structurally-updated JSON.
 *   - Dropping a column removes it from the descriptor.
 *   - Reset button only renders when `dirty && inferred` (otherwise it'd be
 *     a no-op control polluting the focus order).
 *   - Reset button click invokes onReset.
 *
 * The component is pure-presentational — the integration with
 * `<FossilPlayground/>` (auto-inference, dirty-state guard) is exercised
 * E2E in `apps/landing/tests/e2e/csvw-inference.spec.ts` (skip-pending
 * until plan 09-09 ships a curated example without a CSVW descriptor).
 */

import { describe, test, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { CsvwPreview } from '../src/csvw/CsvwPreview.js';
import { applyCsvw } from '../src/csvw/apply.js';
import type { CsvwTable } from '../src/csvw/infer.js';

afterEach(cleanup);

/** Two-column inferred sample (the hello-example shape — id + name). */
const sampleInferred: CsvwTable = {
  '@context': 'http://www.w3.org/ns/csvw',
  url: '@examples/hello.csv',
  tableSchema: {
    columns: [
      { name: 'id', datatype: 'integer' },
      { name: 'name', datatype: 'string' },
    ],
  },
};
const sampleJson = applyCsvw(sampleInferred);

describe('CsvwPreview (PLAY-09 + PLAY-11)', () => {
  test('renders empty state when value is empty', () => {
    render(
      <CsvwPreview
        inferred={null}
        value=""
        onChange={() => {}}
        dirty={false}
        onReset={() => {}}
      />,
    );
    // The placeholder copy is the user-facing hint for what an empty
    // descriptor means + how to populate it (paste a CSV → auto-infer).
    const status = screen.getByRole('status');
    expect(status.textContent ?? '').toMatch(/no csvw descriptor/i);
    expect(status.textContent ?? '').toMatch(/auto-infer/i);
  });

  test('renders each column row with name input + datatype <select>', () => {
    render(
      <CsvwPreview
        inferred={sampleInferred}
        value={sampleJson}
        onChange={() => {}}
        dirty={false}
        onReset={() => {}}
      />,
    );
    // Property assertions (no jest-dom in this workspace).
    expect(
      (screen.getByTestId('csvw-col-name-0') as HTMLInputElement).value,
    ).toBe('id');
    expect(
      (screen.getByTestId('csvw-col-type-0') as HTMLSelectElement).value,
    ).toBe('integer');
    expect(
      (screen.getByTestId('csvw-col-name-1') as HTMLInputElement).value,
    ).toBe('name');
    expect(
      (screen.getByTestId('csvw-col-type-1') as HTMLSelectElement).value,
    ).toBe('string');
  });

  test('editing a column type calls onChange with updated JSON', () => {
    const onChange = vi.fn();
    render(
      <CsvwPreview
        inferred={sampleInferred}
        value={sampleJson}
        onChange={onChange}
        dirty={false}
        onReset={() => {}}
      />,
    );
    fireEvent.change(screen.getByTestId('csvw-col-type-0'), {
      target: { value: 'string' },
    });
    expect(onChange).toHaveBeenCalledTimes(1);
    const newJson = onChange.mock.calls[0]?.[0] as string;
    const parsed = JSON.parse(newJson) as CsvwTable;
    expect(parsed.tableSchema.columns[0]).toEqual({
      name: 'id',
      datatype: 'string',
    });
    // The OTHER column is unchanged — the update is a structural patch, not
    // a wholesale replace. Surfaces a class of bugs where the spread misses
    // a column ("which is which?" off-by-one).
    expect(parsed.tableSchema.columns[1]).toEqual({
      name: 'name',
      datatype: 'string',
    });
  });

  test('dropping a column removes it from the column list', () => {
    const onChange = vi.fn();
    render(
      <CsvwPreview
        inferred={sampleInferred}
        value={sampleJson}
        onChange={onChange}
        dirty={false}
        onReset={() => {}}
      />,
    );
    fireEvent.click(screen.getByTestId('csvw-col-drop-0'));
    expect(onChange).toHaveBeenCalledTimes(1);
    const parsed = JSON.parse(onChange.mock.calls[0]?.[0] as string) as CsvwTable;
    expect(parsed.tableSchema.columns).toHaveLength(1);
    // The remaining column is the one we did NOT drop.
    expect(parsed.tableSchema.columns[0]).toEqual({
      name: 'name',
      datatype: 'string',
    });
  });

  test('Reset button only renders when dirty AND inferred is set', () => {
    const { rerender } = render(
      <CsvwPreview
        inferred={sampleInferred}
        value={sampleJson}
        onChange={() => {}}
        dirty={false}
        onReset={() => {}}
      />,
    );
    // Clean state — no Reset button.
    expect(screen.queryByTestId('csvw-reset')).toBeNull();
    // Dirty + inferred — Reset button renders.
    rerender(
      <CsvwPreview
        inferred={sampleInferred}
        value={sampleJson}
        onChange={() => {}}
        dirty={true}
        onReset={() => {}}
      />,
    );
    expect(screen.getByTestId('csvw-reset')).toBeTruthy();
    // Dirty but no inferred (user typed a descriptor from scratch with no
    // inference ever having run) — Reset button suppressed because there's
    // nothing to reset TO.
    rerender(
      <CsvwPreview
        inferred={null}
        value={sampleJson}
        onChange={() => {}}
        dirty={true}
        onReset={() => {}}
      />,
    );
    expect(screen.queryByTestId('csvw-reset')).toBeNull();
  });

  test('Reset button click invokes onReset exactly once', () => {
    const onReset = vi.fn();
    render(
      <CsvwPreview
        inferred={sampleInferred}
        value={sampleJson}
        onChange={() => {}}
        dirty={true}
        onReset={onReset}
      />,
    );
    fireEvent.click(screen.getByTestId('csvw-reset'));
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  test('region landmark + theme data-attribute wired for axe-core', () => {
    render(
      <CsvwPreview
        inferred={null}
        value=""
        onChange={() => {}}
        dirty={false}
        onReset={() => {}}
        theme="dark"
      />,
    );
    const region = screen.getByTestId('csvw-preview');
    expect(region.getAttribute('role')).toBe('region');
    expect(region.getAttribute('aria-label')).toBe('CSVW descriptor');
    expect(region.getAttribute('data-theme')).toBe('dark');
  });

  test('out-of-canonical-set datatype is preserved in the dropdown', () => {
    // Persisted CSVW carrying a datatype outside the canonical set should
    // NOT be silently snapped to the first option; the dropdown injects an
    // extra <option> so the <select>'s `value` matches a real <option>.
    const exotic: CsvwTable = {
      '@context': 'http://www.w3.org/ns/csvw',
      url: 'x.csv',
      tableSchema: { columns: [{ name: 'flag', datatype: 'gYear' }] },
    };
    render(
      <CsvwPreview
        inferred={null}
        value={applyCsvw(exotic)}
        onChange={() => {}}
        dirty={false}
        onReset={() => {}}
      />,
    );
    const select = screen.getByTestId('csvw-col-type-0') as HTMLSelectElement;
    expect(select.value).toBe('gYear');
  });
});

describe('applyCsvw + parseCsvw round-trip', () => {
  test('applyCsvw → parseCsvw preserves the table shape', async () => {
    const { applyCsvw: apply, parseCsvw: parse } = await import(
      '../src/csvw/apply.js'
    );
    const back = parse(apply(sampleInferred));
    expect(back).toEqual(sampleInferred);
  });

  test('parseCsvw soft-fails on invalid input', async () => {
    const { parseCsvw: parse } = await import('../src/csvw/apply.js');
    expect(parse('not json')).toBeNull();
    expect(parse('{}')).toBeNull();
    expect(parse('null')).toBeNull();
    expect(parse('{"@context":"x"}')).toBeNull();
    expect(
      parse('{"@context":"http://www.w3.org/ns/csvw"}'),
    ).toBeNull();
    expect(
      parse(
        '{"@context":"http://www.w3.org/ns/csvw","tableSchema":{"columns":"not array"}}',
      ),
    ).toBeNull();
  });
});
