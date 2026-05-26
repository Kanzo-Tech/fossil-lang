/**
 * FossilViewer — IDE-style tabs composition around `<FossilGraphView/>`.
 *
 * Four tabs (Phase 12 VIEW-03):
 *   - Graph   — the Cosmos.gl WebGL viewer (or TabularFallback).
 *   - Turtle  — `<TurtleTab/>` rendering the serialized `.ttl` text.
 *   - Vertices — semantic <table> of all input vertices.
 *   - Edges   — semantic <table> of all input edges.
 *
 * Tabs implemented with `@fossil-lang/ui` Tabs primitives (Phase 10 plan
 * 10-03; variant="line" gives the IDE look — underline indicator on the
 * active trigger). Turtle adapter strips the public `id`/`type`/`label`
 * keys and stringifies any remaining vertex props into the
 * `TurtleVertexRow.props` map.
 *
 * Phase 12 plan 12-04 Task 2.
 */

import { useMemo, useState } from 'react';
import type { Selection } from '@uwdata/mosaic-core';

import {
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
} from '@fossil-lang/ui';

import {
  FossilGraphView,
  type FossilGraphViewProps,
} from './FossilGraphView.js';
import { TurtleTab } from './turtle/index.js';
import type {
  TurtleVertexRow,
  TurtleEdgeRow,
} from './turtle/index.js';
import type { VertexRow, EdgeRow } from './hooks/useGraphData.js';

export interface FossilViewerProps {
  vertices: VertexRow[];
  edges: EdgeRow[];
  selection?: Selection | null;
  onSelectVertex?: (v: VertexRow | null) => void;
  webgl?: boolean;
  className?: string;
  /**
   * Turtle prefix map. Defaults to `{ ex, rdf, rdfs, xsd }` (the
   * standard launch-demo trio).
   */
  prefixes?: Record<string, string>;
  defaultTab?: 'graph' | 'turtle' | 'vertices' | 'edges';
}

const DEFAULT_PREFIXES: Record<string, string> = {
  ex: 'http://example.org/',
  rdf: 'http://www.w3.org/1999/02/22-rdf-syntax-ns#',
  rdfs: 'http://www.w3.org/2000/01/rdf-schema#',
  xsd: 'http://www.w3.org/2001/XMLSchema#',
};

const RDF_TYPE_BASE = 'http://example.org/';
const DEFAULT_PREDICATE = 'http://example.org/relatedTo';

/** Coerce a vertex's extra fields (id/type/label stripped) into the
 *  TurtleVertexRow.props shape — strings/numbers/booleans pass through;
 *  null stays null; everything else gets stringified. */
function vertexToTurtleProps(
  v: VertexRow,
): Record<string, string | number | boolean | null> {
  const out: Record<string, string | number | boolean | null> = {};
  for (const [k, val] of Object.entries(v)) {
    if (k === 'id' || k === 'type' || k === 'label') continue;
    if (val === null) {
      out[k] = null;
    } else if (
      typeof val === 'string' ||
      typeof val === 'number' ||
      typeof val === 'boolean'
    ) {
      out[k] = val;
    } else {
      out[k] = String(val);
    }
  }
  return out;
}

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

const tableStyle: React.CSSProperties = {
  borderCollapse: 'collapse',
  width: '100%',
  fontSize: 'var(--fossil-fonts-sizeSmall, 13px)',
};

const captionStyle: React.CSSProperties = {
  textAlign: 'left',
  padding: '0.5rem 0.5rem 0.25rem',
  fontWeight: 600,
  fontSize: 'var(--fossil-fonts-sizeSmall, 13px)',
};

/** Private DataTable helper used by the Vertices + Edges tabs.
 *  Not exported — implementation detail of FossilViewer. */
function DataTable<T>({
  rows,
  columns,
  caption,
  rowKey,
  testid,
}: {
  rows: T[];
  columns: { header: string; render: (row: T) => React.ReactNode }[];
  caption: string;
  rowKey: (row: T, i: number) => string;
  testid: string;
}): JSX.Element {
  if (rows.length === 0) {
    return (
      <p
        style={{
          margin: 0,
          padding: '0.5rem',
          color: 'var(--fossil-colors-mutedForeground, #475569)',
        }}
      >
        No {caption.toLowerCase()}.
      </p>
    );
  }
  return (
    <table style={tableStyle} data-testid={testid}>
      <caption style={captionStyle}>{caption}</caption>
      <thead>
        <tr>
          {columns.map((c) => (
            <th key={c.header} scope="col" style={headerCellStyle}>
              {c.header}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row, i) => (
          <tr key={rowKey(row, i)}>
            {columns.map((c) => (
              <td key={c.header} style={cellStyle}>
                {c.render(row)}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function FossilViewer(props: FossilViewerProps): JSX.Element {
  const [tab, setTab] = useState<string>(props.defaultTab ?? 'graph');
  const prefixes = props.prefixes ?? DEFAULT_PREFIXES;

  // Adapt VertexRow → TurtleVertexRow (strip id/type/label, stringify rest).
  // For type: emit `ex:<type>` so the rowsToTurtle writer's `rdf:type`
  // quad is a fully-qualified IRI per the n3 contract.
  const turtleVertices: TurtleVertexRow[] = useMemo(
    () =>
      props.vertices.map((v) => ({
        iri: v.id.startsWith('http://') || v.id.startsWith('https://')
          ? v.id
          : `${prefixes['ex'] ?? RDF_TYPE_BASE}${encodeURIComponent(v.id)}`,
        type:
          v.type.startsWith('http://') || v.type.startsWith('https://')
            ? v.type
            : `${prefixes['ex'] ?? RDF_TYPE_BASE}${encodeURIComponent(v.type)}`,
        props: vertexToTurtleProps(v),
      })),
    [props.vertices, prefixes],
  );

  const turtleEdges: TurtleEdgeRow[] = useMemo(
    () =>
      props.edges.map((e) => {
        const exNs = prefixes['ex'] ?? RDF_TYPE_BASE;
        const src =
          e.source.startsWith('http://') || e.source.startsWith('https://')
            ? e.source
            : `${exNs}${encodeURIComponent(e.source)}`;
        const dst =
          e.target.startsWith('http://') || e.target.startsWith('https://')
            ? e.target
            : `${exNs}${encodeURIComponent(e.target)}`;
        const pred = e.predicate ?? DEFAULT_PREDICATE;
        return { src, pred, dst };
      }),
    [props.edges, prefixes],
  );

  const graphProps: FossilGraphViewProps = {
    vertices: props.vertices,
    edges: props.edges,
    selection: props.selection,
    onSelectVertex: props.onSelectVertex,
    webgl: props.webgl,
  };

  return (
    <Tabs
      value={tab}
      onValueChange={setTab}
      className={props.className ?? 'fossil-viewer'}
      data-testid="fossil-viewer-root"
    >
      <TabsList variant="line">
        <TabsTrigger value="graph" data-testid="fossil-viewer-tab-graph">
          Graph
        </TabsTrigger>
        <TabsTrigger value="turtle" data-testid="fossil-viewer-tab-turtle">
          Turtle
        </TabsTrigger>
        <TabsTrigger value="vertices" data-testid="fossil-viewer-tab-vertices">
          Vertices ({props.vertices.length})
        </TabsTrigger>
        <TabsTrigger value="edges" data-testid="fossil-viewer-tab-edges">
          Edges ({props.edges.length})
        </TabsTrigger>
      </TabsList>

      <TabsContent value="graph">
        <FossilGraphView {...graphProps} />
      </TabsContent>

      <TabsContent value="turtle">
        <TurtleTab
          vertices={turtleVertices}
          edges={turtleEdges}
          prefixes={prefixes}
        />
      </TabsContent>

      <TabsContent value="vertices">
        <DataTable
          rows={props.vertices}
          columns={[
            { header: 'ID', render: (v) => v.id },
            { header: 'Type', render: (v) => v.type },
            { header: 'Label', render: (v) => v.label },
          ]}
          caption="Vertices"
          rowKey={(v) => v.id}
          testid="fossil-viewer-vertices-table"
        />
      </TabsContent>

      <TabsContent value="edges">
        <DataTable
          rows={props.edges}
          columns={[
            { header: 'Source', render: (e) => e.source },
            { header: 'Target', render: (e) => e.target },
            { header: 'Predicate', render: (e) => e.predicate ?? '' },
          ]}
          caption="Edges"
          rowKey={(e, i) => `${e.source}-${e.target}-${i}`}
          testid="fossil-viewer-edges-table"
        />
      </TabsContent>
    </Tabs>
  );
}
