// Playground entrypoint — fetches the baked example files, registers
// languages with Monaco, mounts the 5 panels (4 Monaco editors + 1 output
// div), starts the LSP client (long-lived fossil-wasm Worker), wires
// the Run button (DuckDB-WASM lazy-load), and wires the ShEx
// `onDidChangeContent` debounced route to `fossil/setTargetShex`.
//
//   07-05 → Monaco editors + monaco-languageclient + LSP Worker
//   07-06 (this plan) → DuckDB-WASM lazy-load + Run wiring + fossil/setTargetShex
//   07-07            → Mosaic / @uwdata/vgplot in the output panel
//   07-08            → Reset button + URL-based state sharing
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
import { runCompiledSql }         from './duckdb/runner';

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
    const csvHandle  = mountCsvPanel(host('panel-csv'), csv);
    const shexHandle = mountShexPanel(host('panel-shex'), shex);
    mountOutputPanel(host('panel-output'), '');

    // 4. Start the LSP client AFTER models exist — the language client
    //    sends `textDocument/didOpen` for every matching document found
    //    at `start()`. The Worker is long-lived (separate from DuckDB
    //    Worker which this plan owns the lifecycle of).
    const lspChannel = await startLspClient();

    // 5. ShEx routing (07-06 / B2) — debounce 300ms then send the model
    //    text to `fossil/setTargetShex`. Non-blocking — bad ShEx just
    //    means the backward-check is skipped on the next fossil/checkAll.
    let shexDebounce: ReturnType<typeof setTimeout> | null = null;
    shexHandle.onDidChangeContent?.(() => {
        if (shexDebounce) clearTimeout(shexDebounce);
        shexDebounce = setTimeout(() => {
            lspChannel.client
                .sendRequest('fossil/setTargetShex', { text: shexHandle.text })
                .catch((err: unknown) => {
                    console.warn('fossil/setTargetShex failed:', err);
                });
        }, 300);
    });

    // 6. Run button (07-06 / SC#2 first half). DuckDB-WASM is
    //    lazy-loaded on the FIRST click (the `@duckdb/duckdb-wasm`
    //    chunk is excluded from `optimizeDeps` so Vite emits it as a
    //    dynamic chunk).
    const runBtn = document.getElementById('btn-run') as HTMLButtonElement | null;
    const progressEl = document.getElementById('progress') as HTMLProgressElement | null;
    if (!runBtn || !progressEl) {
        throw new Error('Run button or progress element missing from index.html');
    }
    runBtn.disabled = false;
    runBtn.addEventListener('click', async () => {
        progressEl.hidden = false;
        progressEl.value = 0;
        try {
            const uri = 'inmemory://playground/main.fossil';
            const compileResult = await lspChannel.client.sendRequest(
                'fossil/compileFile',
                { uri },
            ) as { sql: string; manifest_yaml: string };

            // Runtime shape guard (W-rt-guard) — the TS cast above does
            // NOT validate at runtime. If the Worker returned an
            // unexpected shape (e.g. version skew between worker.js and
            // main.ts changed the response contract), fail loudly here
            // instead of crashing later inside DuckDB SQL parsing with a
            // confusing diagnostic.
            if (
                typeof compileResult?.sql !== 'string'
                || typeof compileResult?.manifest_yaml !== 'string'
            ) {
                throw new Error(
                    'fossil/compileFile returned unexpected shape: '
                    + JSON.stringify(compileResult),
                );
            }

            const result = await runCompiledSql(
                compileResult.sql,
                csvHandle.text,
                (p) => { progressEl.value = p; },
            );
            // 07-07 consumes this CustomEvent and renders via Mosaic.
            window.dispatchEvent(new CustomEvent('fossil:run-result', { detail: result }));
        } catch (e) {
            console.error('run failed:', e);
            // 07-08 polishes the error UX (modal); 07-06 ships an alert().
            alert(`Run failed: ${String(e)}`);
        } finally {
            progressEl.hidden = true;
        }
    });

    console.info('playground: panels + LSP + DuckDB Run wired; Mosaic rendering lands in 07-07');
}

bootstrap().catch((err: unknown) => {
    console.error('playground bootstrap failed:', err);
    const msg = err instanceof Error ? err.message : String(err);
    document.body.insertAdjacentHTML(
        'beforeend',
        `<pre style="color:#a00;padding:1rem;">Bootstrap error: ${msg}</pre>`,
    );
});
