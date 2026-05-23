// Mapping panel — Fossil source (`.fossil`) editor.
//
// In 07-04 this is a placeholder <textarea>. In 07-05 it is replaced by a
// Monaco editor wired to monaco-languageclient + the LSP-over-postMessage
// worker (07-03). The `PanelHandle` shape is forward-compatible: the
// Monaco swap-in will preserve the `text` getter/setter contract so the
// `Reset` button (07-08) and example-load path (this file's mount call)
// keep working.

export type PanelHandle = {
    get text(): string;
    set text(v: string);
    dispose(): void;
};

export function mountMappingPanel(host: HTMLElement, initial: string): PanelHandle {
    const ta = document.createElement('textarea');
    ta.value = initial;
    ta.spellcheck = false;
    ta.setAttribute('aria-label', 'mapping editor (.fossil)');
    ta.setAttribute('data-lang', 'fossil');
    host.replaceChildren(ta);
    return {
        get text() { return ta.value; },
        set text(v: string) { ta.value = v; },
        dispose() { ta.remove(); },
    };
}
