/**
 * TabularFallback — accessible vertices/edges tables for hosts without
 * WebGL (canvas.getContext('webgl2') === null) OR when callers explicitly
 * set `webgl={false}` on `<FossilGraphView/>`.
 *
 * Phase 12 plan 12-04 — discharges VIEW-04 (a11y fallback) and carries
 * forward A11Y-01 from Phase 8 (axe-clean assertion gate). Two semantic
 * `<table>` elements with `<caption>`, `<thead scope="col">`,
 * `<tbody>` — screen readers announce structure correctly; keyboard
 * users can tab through cells.
 *
 * Used by `<FossilGraphView/>` and `<FossilViewer/>` (Vertices + Edges
 * tabs use the same DataTable rendering at the call site).
 *
 * No Tailwind / no icon library — inline styles with `--fossil-*` CSS
 * variables for theme-aware overrides.
 */

import type { VertexRow, EdgeRow } from '../hooks/useGraphData.js';

export interface TabularFallbackProps {
  vertices: VertexRow[];
  edges: EdgeRow[];
  className?: string;
}

const tableStyle: React.CSSProperties = {
  borderCollapse: 'collapse',
  width: '100%',
  fontSize: 'var(--fossil-fonts-sizeSmall, 13px)',
};

const cellStyle: React.CSSProperties = {
  padding: '0.25rem 0.5rem',
  borderBottom: '1px solid var(--fossil-colors-border, #e2e8f0)',
  textAlign: 'left',
};

const headerCellStyle: React.CSSProperties = {
  ...cellStyle,
  fontWeight: 600,
  borderBottom: '2px solid var(--fossil-colors-border, #e2e8f0)',
};

const captionStyle: React.CSSProperties = {
  textAlign: 'left',
  padding: '0.5rem 0.5rem 0.25rem',
  fontWeight: 600,
  fontSize: 'var(--fossil-fonts-sizeSmall, 13px)',
};

export function TabularFallback({
  vertices,
  edges,
  className,
}: TabularFallbackProps): JSX.Element {
  return (
    <section
      aria-label="Graph data tabular fallback"
      className={className ?? 'fossil-viewer-tabular-fallback'}
      data-testid="fossil-viewer-tabular-fallback"
      style={{ padding: '0.5rem', overflow: 'auto' }}
    >
      {vertices.length === 0 ? (
        <p style={{ margin: 0, color: 'var(--fossil-colors-mutedForeground, #475569)' }}>
          No vertices.
        </p>
      ) : (
        <table style={tableStyle} data-testid="fossil-viewer-vertices-table">
          <caption style={captionStyle}>Vertices</caption>
          <thead>
            <tr>
              <th scope="col" style={headerCellStyle}>ID</th>
              <th scope="col" style={headerCellStyle}>Type</th>
              <th scope="col" style={headerCellStyle}>Label</th>
            </tr>
          </thead>
          <tbody>
            {vertices.map((v) => (
              <tr key={v.id}>
                <td style={cellStyle}>{v.id}</td>
                <td style={cellStyle}>{v.type}</td>
                <td style={cellStyle}>{v.label}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {edges.length === 0 ? (
        <p style={{ margin: '0.5rem 0 0', color: 'var(--fossil-colors-mutedForeground, #475569)' }}>
          No edges.
        </p>
      ) : (
        <table
          style={{ ...tableStyle, marginTop: '0.75rem' }}
          data-testid="fossil-viewer-edges-table"
        >
          <caption style={captionStyle}>Edges</caption>
          <thead>
            <tr>
              <th scope="col" style={headerCellStyle}>Source</th>
              <th scope="col" style={headerCellStyle}>Target</th>
              <th scope="col" style={headerCellStyle}>Predicate</th>
            </tr>
          </thead>
          <tbody>
            {edges.map((e, i) => (
              <tr key={`${e.source}-${e.target}-${i}`}>
                <td style={cellStyle}>{e.source}</td>
                <td style={cellStyle}>{e.target}</td>
                <td style={cellStyle}>{e.predicate ?? ''}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
