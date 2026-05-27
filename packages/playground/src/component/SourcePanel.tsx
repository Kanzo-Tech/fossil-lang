/**
 * SourcePanel — read-only DuckDB DESCRIBE preview of resolved sources.
 *
 * Lives inside the IDE tabs layout's left panel (TabsContent value="source").
 * Pure presentational — receives the schemas snapshot via props; the
 * parent component owns the state and populates it from the
 * `useInferredDescriptors.introspectAndRegister(...)` return value (Phase 14
 * plan 14-03 widens the hook's API to also return the captured schemas).
 *
 * When the mapping has no `io.csv("...")` / `io.json("...")` source bindings,
 * the panel renders a hint pointing the user at the Mapping tab. When the
 * introspection step is in flight, it renders a polite live region so
 * screen readers announce the loading state.
 *
 * Phase 14 plan 14-03.
 */

export interface SourceColumn {
  name: string;
  primitive: string;
}

export interface SourceSchema {
  sourceName: string;
  columns: SourceColumn[];
}

export interface SourcePanelProps {
  /**
   * Snapshot of the schemas from the last successful introspectAndRegister
   * call. Empty array = either no sources detected OR introspection has
   * not yet run for the current mapping.
   */
  schemas: SourceSchema[];
  /** True while introspection is in flight (Run handler holding the lock). */
  loading?: boolean;
}

export function SourcePanel(props: SourcePanelProps): JSX.Element {
  const { schemas, loading } = props;
  if (loading) {
    return (
      <div
        role="status"
        aria-live="polite"
        data-testid="source-panel"
        style={{ padding: '1rem', color: 'var(--fossil-colors-muted)' }}
      >
        Introspecting sources…
      </div>
    );
  }
  if (schemas.length === 0) {
    return (
      <div
        data-testid="source-panel"
        style={{ padding: '1rem', color: 'var(--fossil-colors-muted)' }}
      >
        No sources detected. Type{' '}
        <code>io.csv(&quot;@connector/path&quot;)</code> in the Mapping panel
        and click Run to see the inferred schema here.
      </div>
    );
  }
  return (
    <section
      aria-label="Source schemas"
      data-testid="source-panel"
      style={{ padding: '1rem', overflow: 'auto' }}
    >
      {schemas.map((s) => (
        <article key={s.sourceName} style={{ marginBottom: '1rem' }}>
          <h3
            style={{
              fontSize: 'var(--fossil-fonts-size-small, 11px)',
              fontFamily: 'var(--fossil-fonts-mono, monospace)',
              margin: '0 0 0.25rem 0',
            }}
          >
            {s.sourceName}
          </h3>
          <table
            style={{
              borderCollapse: 'collapse',
              fontSize: 'var(--fossil-fonts-size-small, 11px)',
            }}
          >
            <thead>
              <tr>
                <th
                  scope="col"
                  style={{ textAlign: 'left', paddingRight: '1rem' }}
                >
                  Column
                </th>
                <th scope="col" style={{ textAlign: 'left' }}>
                  Type
                </th>
              </tr>
            </thead>
            <tbody>
              {s.columns.map((c) => (
                <tr key={c.name}>
                  <td
                    style={{
                      paddingRight: '1rem',
                      fontFamily: 'var(--fossil-fonts-mono, monospace)',
                    }}
                  >
                    {c.name}
                  </td>
                  <td>{c.primitive}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </article>
      ))}
    </section>
  );
}
