/**
 * ResultTable — plain-HTML tabular renderer for vertex/edge result rows.
 *
 * Phase 8 v0.1 deliberately ships a no-dependency `<table>` instead of the
 * heavier `@tanstack/react-table` referenced in CONTEXT.md. Justification:
 *
 *   - Default playground results are small (the walking-skeleton hello
 *     example emits ~5 vertices + 5 edges). Sort/filter/pagination affordances
 *     are deferred to Phase 9 PLAY-09/PLAY-11.
 *   - Plays well with the A11Y-01 fallback contract — a semantic `<table>`
 *     with `<caption>` + `<th scope="col">` satisfies axe-core out of the
 *     box; the same DOM shape that the ResultGraph fallback path renders.
 *   - Avoids pulling @tanstack/react-table into the core gzip budget at a
 *     point in the timeline (08-09) where the actual rendering needs are
 *     trivial. If/when Phase 9 grows the result panel (sortable columns,
 *     paginated rows, column type inference), swap to @tanstack/react-table
 *     in a follow-up plan — the props shape is intentionally compatible.
 *
 * Per RESEARCH.md Pitfall 8 (a11y of result widgets): every cell renders as
 * a string via `String(value)` so React doesn't choke on Arrow's
 * BigInt/Date/etc. values. Future polish can specialise per-type rendering.
 */

import type { ReactNode } from 'react';

export interface ResultTableProps<TRow extends Record<string, unknown>> {
  /** Row data. Empty array renders an a11y-friendly "no results" status. */
  rows: TRow[];
  /** Optional accessible caption (e.g., "Vertices", "Edges"). */
  caption?: string;
  /** Optional explicit column order. Defaults to keys of the first row. */
  columns?: Array<keyof TRow & string>;
  /** Optional class for theme hooks. */
  className?: string;
}

export function ResultTable<TRow extends Record<string, unknown>>({
  rows,
  caption,
  columns,
  className,
}: ResultTableProps<TRow>): JSX.Element {
  if (rows.length === 0) {
    return (
      <div role="status" aria-live="polite" className={className}>
        {caption ? `${caption}: no results.` : 'No results.'}
      </div>
    );
  }

  const cols: string[] =
    columns ?? Object.keys(rows[0] as Record<string, unknown>);

  return (
    <table className={className ?? 'fossil-result-table'}>
      {caption && <caption>{caption}</caption>}
      <thead>
        <tr>
          {cols.map((c) => (
            <th key={c} scope="col">
              {c}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row, i) => (
          <tr key={i}>
            {cols.map((c) => (
              <td key={c}>{renderCell((row as Record<string, unknown>)[c])}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function renderCell(value: unknown): ReactNode {
  if (value === null || value === undefined) return '';
  if (typeof value === 'object') return JSON.stringify(value);
  // BigInt + Date + symbol etc. -> String() -> render text content.
  return String(value);
}
