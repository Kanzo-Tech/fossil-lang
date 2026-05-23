// ShEx shape panel — plaintext editor. No LSP awareness in v0.1.

import type { PanelHandle } from './mapping-panel';

export function mountShexPanel(host: HTMLElement, initial: string): PanelHandle {
    const ta = document.createElement('textarea');
    ta.value = initial;
    ta.spellcheck = false;
    ta.setAttribute('aria-label', 'ShEx shape editor');
    ta.setAttribute('data-lang', 'shex');
    host.replaceChildren(ta);
    return {
        get text() { return ta.value; },
        set text(v: string) { ta.value = v; },
        dispose() { ta.remove(); },
    };
}
