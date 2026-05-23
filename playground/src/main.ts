// Playground entrypoint — fetches the baked example files, registers
// languages with Monaco, mounts the 5 panels (4 Monaco editors + 1 output
// div), and starts the LSP client (which spawns the long-lived
// fossil-wasm Worker).
//
//   07-05 (this plan) → Monaco editors + monaco-languageclient + LSP Worker
//   07-06 (next)      → DuckDB-WASM lazy-load + Run wiring + fossil/setTargetShex
//   07-07             → Mosaic / @uwdata/vgplot in the output panel
//   07-08             → Reset button + URL-based state sharing
//
// Example fetch URLs resolve against `public/examples/*` which Vite serves
// at `/examples/...` (07-04 convention).

import { mountMappingPanel } from './panels/mapping-panel';
import { mountCsvwPanel }    from './panels/csvw-panel';
import { mountCsvPanel }     from './panels/csv-panel';
import { mountShexPanel }    from './panels/shex-panel';
import { mountOutputPanel }  from './panels/output-panel';
import { registerFossilLanguage } from './lsp/fossil-lang';
import { registerShexLanguage }   from './lsp/shex-lang';
import { startLspClient }         from './lsp/client';

async function fetchText(path: string, fallback: string): Promise<string> {
    try {
        const r = await fetch(path);
        if (!r.ok) return fallback;
        return await r.text();
    } catch {
        return fallback;
    }
}

async function bootstrap(): Promise<void> {
    // 1. Language registrations — must happen BEFORE any Monaco model is
    //    created with these language ids (Monaco refuses unregistered ids).
    registerFossilLanguage();
    registerShexLanguage();

    // 2. Load default-example texts (07-04 convention).
    const [mapping, csvw, csv, shex] = await Promise.all([
        fetchText('/examples/hello.fossil',    ''),
        fetchText('/examples/hello.csvw.json', ''),
        fetchText('/examples/hello.csv',       ''),
        fetchText('/examples/hello.shex',      ''),
    ]);

    // 3. Mount panels — 4 Monaco editors + 1 output div (Mosaic in 07-07).
    const host = (id: string): HTMLElement => {
        const el = document.querySelector<HTMLElement>(`#${id} .editor, #${id} .output`);
        if (!el) throw new Error(`missing host element for #${id}`);
        return el;
    };
    mountMappingPanel(host('panel-mapping'), mapping);
    mountCsvwPanel(host('panel-csvw'), csvw);
    mountCsvPanel(host('panel-csv'), csv);
    mountShexPanel(host('panel-shex'), shex);
    mountOutputPanel(host('panel-output'), '');

    // 4. Start the LSP client AFTER models exist — the language client
    //    sends `textDocument/didOpen` for every matching document found
    //    at `start()`. The Worker is long-lived (separate from DuckDB
    //    Worker which 07-06 owns).
    await startLspClient();

    console.info('playground: panels + LSP mounted; DuckDB Run lands in 07-06');
}

bootstrap().catch((err: unknown) => {
    console.error('playground bootstrap failed:', err);
    const msg = err instanceof Error ? err.message : String(err);
    document.body.insertAdjacentHTML(
        'beforeend',
        `<pre style="color:#a00;padding:1rem;">Bootstrap error: ${msg}</pre>`,
    );
});
