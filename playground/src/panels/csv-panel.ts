// CSV data panel — plaintext editor. No LSP / Monarch syntax in v0.1.

import * as monaco from 'monaco-editor';
import type { PanelHandle } from './types';

export function mountCsvPanel(host: HTMLElement, initial: string): PanelHandle {
    const uri = monaco.Uri.parse('inmemory://playground/data.csv');
    const existing = monaco.editor.getModel(uri);
    const model = existing ?? monaco.editor.createModel(initial, 'plaintext', uri);
    if (existing && existing.getValue() !== initial) existing.setValue(initial);

    const editor = monaco.editor.create(host, {
        model,
        theme: 'vs-dark',
        automaticLayout: true,
        minimap: { enabled: false },
        fontSize: 13,
        scrollBeyondLastLine: false,
    });

    return {
        get text(): string { return model.getValue(); },
        set text(v: string) { model.setValue(v); },
        dispose(): void { editor.dispose(); model.dispose(); },
    };
}
