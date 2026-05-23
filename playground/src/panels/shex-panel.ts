// ShEx shape panel — uses the minimal Monarch tokenizer registered by
// `playground/src/lsp/shex-lang.ts`. 07-06 wires `onDidChangeContent`
// so `playground/src/main.ts` can debounce-route ShEx edits through the
// `fossil/setTargetShex` custom LSP request (B2 fix — closes the
// 07-05-deferred ShEx browser-routing gap).

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
        /**
         * 07-06 / B2: forward Monaco's `onDidChangeContent` to the caller.
         * `main.ts` debounces this (300ms) and sends the model text to
         * `fossil/setTargetShex`. Returns a disposer that detaches the
         * Monaco subscription.
         */
        onDidChangeContent(cb: () => void): () => void {
            const sub = model.onDidChangeContent(() => cb());
            return () => sub.dispose();
        },
    };
}
