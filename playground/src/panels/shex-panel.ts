// ShEx shape panel — uses the minimal Monarch tokenizer registered by
// `playground/src/lsp/shex-lang.ts`. The `fossil/setTargetShex` LSP custom
// request (debounced send of model content) is wired in 07-06 Task 2.

import * as monaco from 'monaco-editor';
import type { PanelHandle } from './types';

export function mountShexPanel(host: HTMLElement, initial: string): PanelHandle {
    const uri = monaco.Uri.parse('inmemory://playground/shape.shex');
    const existing = monaco.editor.getModel(uri);
    const model = existing ?? monaco.editor.createModel(initial, 'shex', uri);
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
