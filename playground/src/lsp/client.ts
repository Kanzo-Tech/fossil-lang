// monaco-languageclient setup + the diagnostics fanout (B1 fix —
// exporting side).
//
// monaco-languageclient v10.7 has strict peer-dep requirements (Pitfall 7):
// peer-deps are pinned exactly in `playground/package.json`:
//   - monaco-editor@0.52.0
//   - monaco-languageclient@10.7.0
//   - vscode-languageclient@9.0.1
// Changing any of the three requires a cross-check against the v10.7
// changelog.
//
// The Worker is spawned with Vite's `new Worker(new URL('./worker.ts',
// import.meta.url), { type: 'module' })` pattern — Vite emits the Worker
// as a separate chunk and rewrites the URL at build time.

import { BrowserMessageReader, BrowserMessageWriter } from 'vscode-languageserver-protocol/browser';
import { MonacoLanguageClient } from 'monaco-languageclient';
import { CloseAction, ErrorAction } from 'vscode-languageclient';

export type DiagnosticsListener = () => void;

export interface LspChannel {
    worker: Worker;
    client: MonacoLanguageClient;
    /**
     * Subscribe to `textDocument/publishDiagnostics` notifications.
     * Returns an `off()` disposer.
     *
     * NOTE: the actual `client.onNotification('textDocument/publishDiagnostics', …)`
     * wire-up that invokes the fanout lives in 07-10 Task 2 (perf spec).
     * This file exports the TYPE + a placeholder fanout-array so the
     * `LspChannel` shape is final from 07-05 onward.
     */
    onDiagnostics(cb: DiagnosticsListener): () => void;
    dispose(): Promise<void>;
}

export async function startLspClient(): Promise<LspChannel> {
    const worker = new Worker(
        new URL('./worker.ts', import.meta.url),
        { type: 'module', name: 'fossil-lsp-worker' },
    );
    const reader = new BrowserMessageReader(worker);
    const writer = new BrowserMessageWriter(worker);

    const client = new MonacoLanguageClient({
        name: 'Fossil Language Client',
        clientOptions: {
            documentSelector: [{ language: 'fossil' }],
            errorHandler: {
                error: () => ({ action: ErrorAction.Continue }),
                closed: () => ({ action: CloseAction.DoNotRestart }),
            },
        },
        messageTransports: { reader, writer },
    });
    await client.start();

    // Diagnostics fanout. 07-10 Task 2 wires the actual
    // monaco-languageclient onNotification('textDocument/publishDiagnostics')
    // to invoke `__fireDiagnostics()`. The fanout-array lives here so the
    // LspChannel shape is stable from 07-05 onward.
    const listeners = new Set<DiagnosticsListener>();
    function onDiagnostics(cb: DiagnosticsListener): () => void {
        listeners.add(cb);
        return () => { listeners.delete(cb); };
    }
    // Exposed for 07-10 to call from the wire-up site:
    (client as unknown as { __fireDiagnostics?: () => void }).__fireDiagnostics = () => {
        for (const cb of listeners) {
            try { cb(); } catch { /* swallow listener errors */ }
        }
    };

    return {
        worker,
        client,
        onDiagnostics,
        async dispose(): Promise<void> {
            await client.stop();
            worker.terminate();
        },
    };
}
