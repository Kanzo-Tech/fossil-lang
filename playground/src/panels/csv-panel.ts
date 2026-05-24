// CSV data panel — plaintext editor. No LSP / Monarch syntax in v0.1.
//
// 07-08: belt-and-suspenders 10MB refusal on paste. The load-bearing
// check lives in main.ts's Run handler (so drag-drop / programmatic
// model.setValue / refresh-from-URL all hit the cap); this paste-time
// check just gives faster feedback to the most common interaction.
//
// We listen on Monaco's model.onDidChangeContent. When the post-edit
// content exceeds CSV_LIMIT_BYTES, we roll back to the prior content
// and surface the refusal modal. Rolling back inside onDidChangeContent
// is safe because Monaco re-fires the event on `pushEditOperations`,
// so we guard with a `reverting` flag to avoid recursion.

import * as monaco from 'monaco-editor';
import type { PanelHandle } from './types';
import {
    assertWithinLimit,
    showCsvLimitModal,
    CsvTooLargeError,
} from '../limits/csv-size';

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

    // 07-08: paste/edit-time refusal. Track last known-good text so we
    // can roll back without re-reading the model (the post-paste model
    // value IS the over-limit text).
    let lastGood = model.getValue();
    let reverting = false;
    const sub = model.onDidChangeContent(() => {
        if (reverting) return;
        const next = model.getValue();
        try {
            assertWithinLimit(next);
            lastGood = next;
        } catch (e) {
            if (e instanceof CsvTooLargeError) {
                reverting = true;
                model.setValue(lastGood);
                reverting = false;
                void showCsvLimitModal(e);
                return;
            }
            throw e;
        }
    });

    return {
        get text(): string { return model.getValue(); },
        set text(v: string) {
            // Programmatic sets (e.g. reset flow) bypass the paste guard.
            // The Run handler's assertWithinLimit is still the load-bearing
            // check for whatever ends up in the model.
            reverting = true;
            model.setValue(v);
            reverting = false;
            lastGood = v;
        },
        dispose(): void {
            sub.dispose();
            editor.dispose();
            model.dispose();
        },
    };
}
