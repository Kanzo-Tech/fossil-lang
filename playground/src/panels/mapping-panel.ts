// Mapping panel — Fossil source (`.fossil`) editor.
//
// 07-05: replaced the 07-04 placeholder <textarea> with a Monaco editor
// wired to monaco-languageclient + the LSP-over-postMessage Worker
// (07-03 + 07-05/client.ts). The PanelHandle.text contract is preserved.
//
// The model URI matches the LSP `textDocument/didOpen` URI so
// monaco-languageclient routes events to the correct file inside the
// fossil-wasm Workspace (`inmemory://playground/main.fossil`).

import * as monaco from 'monaco-editor';
import type { PanelHandle } from './types';

export type { PanelHandle } from './types';

export function mountMappingPanel(host: HTMLElement, initial: string): PanelHandle {
    const uri = monaco.Uri.parse('inmemory://playground/main.fossil');
    const existing = monaco.editor.getModel(uri);
    const model = existing ?? monaco.editor.createModel(initial, 'fossil', uri);
    if (existing && existing.getValue() !== initial) existing.setValue(initial);

    const editor = monaco.editor.create(host, {
        model,
        theme: 'vs-dark',
        automaticLayout: true,
        minimap: { enabled: false },
        fontSize: 13,
        scrollBeyondLastLine: false,
        // Required for the LSP-returned semantic tokens to render.
        'semanticHighlighting.enabled': true,
    });

    return {
        get text(): string { return model.getValue(); },
        set text(v: string) { model.setValue(v); },
        dispose(): void { editor.dispose(); model.dispose(); },
    };
}
