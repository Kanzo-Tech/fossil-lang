// CSVW descriptor panel — JSON editor. No LSP awareness in v0.1.
// Monaco's built-in JSON mode lands with 07-05's editor swap-in.

import type { PanelHandle } from './mapping-panel';

export function mountCsvwPanel(host: HTMLElement, initial: string): PanelHandle {
    const ta = document.createElement('textarea');
    ta.value = initial;
    ta.spellcheck = false;
    ta.setAttribute('aria-label', 'CSVW descriptor editor (JSON)');
    ta.setAttribute('data-lang', 'json');
    host.replaceChildren(ta);
    return {
        get text() { return ta.value; },
        set text(v: string) { ta.value = v; },
        dispose() { ta.remove(); },
    };
}
