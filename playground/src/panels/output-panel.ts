// Output panel — hosts the vertex+edge table render and listens for
// the `fossil:run-result` CustomEvent dispatched by the Run pipeline
// (07-06 / main.ts).
//
// 07-04 mounted a placeholder text div. 07-07 (this rewrite) replaces
// that with a `.mosaic-target` container and wires the CustomEvent
// listener. The `PanelHandle` `text` slot is a no-op for this panel
// (the output panel is not user-editable); `dispose()` removes the
// CustomEvent listener so re-mount / test scenarios don't leak.
//
// vgplot drift (Task 1): the plan documented a plain-`<table>` fallback
// when the installed @uwdata/vgplot 0.10 surface drifted from the
// intent-shape; we took the fallback. The CustomEvent + render-target
// contract is unchanged regardless.

import type { PanelHandle } from './types';
import { renderVertexEdge } from '../viz/mosaic';
import type * as Arrow from 'apache-arrow';

export interface RunResult {
    vertices: Arrow.Table;
    edges: Arrow.Table;
    rowCount: number;
}

export function mountOutputPanel(host: HTMLElement, _initial: string): PanelHandle {
    host.replaceChildren(); // clear any 07-04 placeholder

    const container = document.createElement('div');
    container.className = 'mosaic-target';
    container.setAttribute('data-panel', 'output');
    container.setAttribute('aria-label', 'output graph');
    host.appendChild(container);

    // Initial placeholder copy (replaced on first `fossil:run-result`).
    const placeholder = document.createElement('p');
    placeholder.className = 'empty-graph';
    placeholder.textContent = 'Click Run to compile + execute the mapping.';
    container.appendChild(placeholder);

    const onResult = (e: Event): void => {
        const ce = e as CustomEvent<RunResult>;
        try {
            renderVertexEdge(container, ce.detail.vertices, ce.detail.edges);
        } catch (err) {
            console.error('mosaic render failed:', err);
            container.replaceChildren();
            const p = document.createElement('p');
            p.style.color = '#a00';
            p.textContent = `Render error: ${String(err)}`;
            container.appendChild(p);
        }
    };
    window.addEventListener('fossil:run-result', onResult);

    return {
        get text() { return ''; },
        set text(_v) { /* no-op — output panel is not editable */ },
        dispose() {
            window.removeEventListener('fossil:run-result', onResult);
            container.remove();
        },
    };
}
