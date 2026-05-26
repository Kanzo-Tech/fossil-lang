/**
 * TurtleTab — renders the post-Run materialized triples as Turtle text.
 *
 * Moved from `packages/playground/src/turtle/TurtleTab.tsx` to
 * `@fossil-lang/viewer` in Phase 12 plan 12-04 — single-source for the
 * view layer. `@fossil-lang/playground` re-exports for v0.1.x compat.
 *
 * Foundation for PLAY-10 ("Turtle" tab next to Graph + Edges table). KGC
 * users verify the graph in Turtle FIRST (the graph viz is the wow, but the
 * Turtle is what makes it credible) — surfacing the serialized triples is
 * the launch-demo "show me the actual RDF" affordance.
 *
 * Consumes `rowsToTurtle(vertices, edges, prefixes)` from plan 09-04 — n3-
 * backed synchronous TTL writer with proper literal escaping + xsd datatype
 * derivation. Caller supplies the prefix table (typically: defaults `ex`,
 * `rdf`, `rdfs`, `xsd` merged with any `@prefix` declarations from the
 * source `.fossil`).
 *
 * a11y per A11Y-01 (axe-core gate must not regress):
 *   - role="tabpanel" + aria-label="Turtle representation" so SR users hear
 *     the panel as a tab content region;
 *   - <pre tabIndex={0}> so keyboard users can focus the source for
 *     selection + screen-reader reading;
 *   - Copy button with aria-label="Copy Turtle to clipboard"
 *     (vs visible "Copy" / "Copied!" text — accessible name stays stable
 *     when the visible text flips, mirrors ARIA_LABELS pattern from
 *     `packages/playground/src/a11y/index.ts`).
 *
 * Theming: `data-theme` attribute drives CSS scope (the surrounding
 * `<FossilPlayground/>` passes `resolvedTheme`).
 */

import { useMemo, useState } from 'react';
import { rowsToTurtle, type VertexRow, type EdgeRow } from './render.js';

export interface TurtleTabProps {
  /** Vertex rows in the rowsToTurtle shape — caller adapts from the
   *  component's VertexRow if needed (see FossilPlayground integration). */
  vertices: VertexRow[];
  /** Edge rows in the rowsToTurtle shape. */
  edges: EdgeRow[];
  /**
   * Prefix map for the Turtle writer (`@prefix` block). Caller passes the
   * source `.fossil`'s declared prefixes merged with defaults (`ex`, `rdf`,
   * `rdfs`, `xsd`). The writer does NOT auto-discover prefixes from IRIs.
   */
  prefixes: Record<string, string>;
  /** Theme — light/dark — driving the data-theme CSS scope. */
  theme?: 'light' | 'dark';
}

/**
 * Tab panel rendering the serialized Turtle + a Copy-to-clipboard button.
 * Pure presentational + clipboard side-effect; no DuckDB / WASM access.
 */
export function TurtleTab({
  vertices,
  edges,
  prefixes,
  theme = 'light',
}: TurtleTabProps): JSX.Element {
  // Memoise the serialization so we don't pay the n3 writer cost on every
  // parent re-render (the parent's vertex/edge arrays change only on Run).
  const ttl = useMemo(
    () => rowsToTurtle(vertices, edges, prefixes),
    [vertices, edges, prefixes],
  );
  const [copied, setCopied] = useState<boolean>(false);

  const onCopy = async (): Promise<void> => {
    // SSR-safe + jsdom-safe: navigator.clipboard is undefined in some
    // headless environments. The Vitest happy-dom suite would otherwise
    // throw on the missing API — guard and bail.
    if (typeof navigator === 'undefined' || !navigator.clipboard) return;
    try {
      await navigator.clipboard.writeText(ttl);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API can reject in non-secure contexts or when permission
      // is denied. Surface a brief "Copied!" → "Copy" no-op rather than
      // crashing; the user can fall back to manual selection of the <pre>.
    }
  };

  return (
    <div
      role="tabpanel"
      aria-label="Turtle representation"
      data-testid="turtle-tab"
      data-theme={theme}
      className="fossil-turtle-tab"
    >
      <button
        type="button"
        onClick={() => {
          void onCopy();
        }}
        aria-label="Copy Turtle to clipboard"
        data-testid="turtle-copy"
        className="fossil-turtle-copy"
      >
        {copied ? 'Copied!' : 'Copy'}
      </button>
      <pre
        tabIndex={0}
        aria-label="Turtle source"
        className="fossil-turtle-source"
        data-testid="turtle-source"
      >
        <code>{ttl}</code>
      </pre>
    </div>
  );
}
