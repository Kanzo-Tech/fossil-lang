// Output panel — graph visualization host.
//
// In 07-04 the body is a <div> with a placeholder message. 07-07 mounts
// Mosaic / @uwdata/vgplot here once the DuckDB run results land. The
// `PanelHandle` `text` slot stores a status message in the meantime
// (e.g. "Run pending" / "Compiling…" / "Error: …").

import type { PanelHandle } from './mapping-panel';

export function mountOutputPanel(host: HTMLElement, _initial: string): PanelHandle {
    const div = document.createElement('div');
    div.setAttribute('aria-label', 'output graph');
    div.setAttribute('data-panel', 'output');
    div.textContent = 'Click Run to compile + execute the mapping (07-06).';
    host.replaceChildren(div);
    return {
        get text() { return div.textContent ?? ''; },
        set text(v: string) { div.textContent = v; },
        dispose() { div.remove(); },
    };
}
