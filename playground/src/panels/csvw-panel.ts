// CSVW descriptor panel — JSON editor. No LSP awareness in v0.1 (Monaco's
// built-in JSON tokenizer handles syntax highlighting).

import * as monaco from 'monaco-editor';
import type { PanelHandle } from './types';

export function mountCsvwPanel(host: HTMLElement, initial: string): PanelHandle {
    const uri = monaco.Uri.parse('inmemory://playground/descriptor.json');
    const existing = monaco.editor.getModel(uri);
    const model = existing ?? monaco.editor.createModel(initial, 'json', uri);
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
