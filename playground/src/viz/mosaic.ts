// Vertex+edge table renderer for the Output panel.
//
// The plan (07-07) tabled @uwdata/vgplot 0.10 with a *documented fallback*:
// if the installed vgplot 0.10 API surface does not export the call shape
// the plan's intent-shape used (`vg.tableMark` + `vg.title`), fall back to
// plain `<table>` rendering. We took the fallback. Probe (Task 1):
//
//   node -e 'import("@uwdata/vgplot").then(m => …)' →
//     - `table`     : function   (BUT requires a Mosaic DuckDB connector)
//     - `tableMark` : undefined  ← plan's intent-shape NOT exported
//     - `title`     : undefined  ← plan's intent-shape NOT exported
//     - `from`      : function   (inline-data helper — exported)
//     - `vconcat`   : function
//     - `plot`      : function
//     - `coordinator` : function
//
// vgplot's `table` is a Mosaic Input that wires through a DuckDB
// coordinator + named SQL table — using it would require either spawning
// a SECOND DuckDB-WASM coordinator (forbidden by the plan / research
// anti-pattern) or registering the Arrow vertex/edge tables into the
// 07-06 DuckDB connection AND bridging it as a Mosaic coordinator. Both
// paths are heavier than the SC#2 wording demands ("vertex+edge tables
// via Mosaic"); the plan explicitly authorises a plain-`<table>` fallback
// with the trade-off documented and a Phase 8 follow-up filed.
//
// Trade-off accepted: PLAY-03 is discharged in spirit (vertex+edge tables
// render in <5s; empty-result UX graceful; re-run clears prior render).
// Mosaic wiring proper can land in Phase 8 alongside the network-view
// differentiator if needed.

import type * as Arrow from 'apache-arrow';

function arrowToObjects(table: Arrow.Table): Record<string, unknown>[] {
    const rows: Record<string, unknown>[] = [];
    for (let i = 0; i < table.numRows; i++) {
        const r: Record<string, unknown> = {};
        for (const f of table.schema.fields) {
            const col = table.getChild(f.name);
            r[f.name] = col?.get(i) ?? null;
        }
        rows.push(r);
    }
    return rows;
}

function renderOneTable(
    parent: HTMLElement,
    title: string,
    columns: string[],
    rows: Record<string, unknown>[],
): void {
    const section = document.createElement('section');
    section.className = 'vx-table';

    const h = document.createElement('h3');
    h.textContent = `${title} (${rows.length})`;
    section.appendChild(h);

    if (rows.length === 0) {
        const note = document.createElement('p');
        note.className = 'empty-graph';
        note.textContent = '(no rows)';
        section.appendChild(note);
        parent.appendChild(section);
        return;
    }

    const tableEl = document.createElement('table');
    const thead = document.createElement('thead');
    const headerRow = document.createElement('tr');
    for (const c of columns) {
        const th = document.createElement('th');
        th.textContent = c;
        headerRow.appendChild(th);
    }
    thead.appendChild(headerRow);
    tableEl.appendChild(thead);

    const tbody = document.createElement('tbody');
    for (const r of rows) {
        const tr = document.createElement('tr');
        for (const c of columns) {
            const td = document.createElement('td');
            const v = r[c];
            td.textContent = v == null ? '' : String(v);
            tr.appendChild(td);
        }
        tbody.appendChild(tr);
    }
    tableEl.appendChild(tbody);
    section.appendChild(tableEl);
    parent.appendChild(section);
}

/**
 * Render vertex + edge Arrow tables into `target`. Clears any previous
 * render first (visual-side hygiene; the DuckDB-Worker reset lives in
 * lifecycle.ts).
 *
 * SC#2 (Phase 7) — "vertex+edge tables via Mosaic". The current
 * implementation uses a plain-`<table>` fallback per the plan's
 * authorised hedge (see header comment for the @uwdata/vgplot 0.10 API
 * surface that drove the choice). The CustomEvent contract is unchanged
 * — 07-08 reset wiring and 07-10's Playwright spec see the same DOM
 * structure (a `.mosaic-target` host with `.vx-table` children OR a
 * `.empty-graph` note).
 */
export function renderVertexEdge(
    target: HTMLElement,
    vertices: Arrow.Table,
    edges: Arrow.Table,
): void {
    target.replaceChildren();

    if (vertices.numRows === 0 && edges.numRows === 0) {
        const note = document.createElement('p');
        note.className = 'empty-graph';
        note.textContent = 'No triples produced. Edit the mapping and try again.';
        target.appendChild(note);
        return;
    }

    const vertexRows = arrowToObjects(vertices);
    const edgeRows   = arrowToObjects(edges);

    const vertexCols = vertices.schema.fields.map(f => f.name);
    const edgeCols   = edges.schema.fields.map(f => f.name);

    renderOneTable(target, 'Vertices', vertexCols, vertexRows);
    renderOneTable(target, 'Edges',    edgeCols,   edgeRows);
}
