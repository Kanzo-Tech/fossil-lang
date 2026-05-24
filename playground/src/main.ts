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
import {
    assertWithinLimit,
    showCsvLimitModal,
    CsvTooLargeError,
} from './limits/csv-size';
import { resetPlayground } from './lifecycle/reset';

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
    // 07-08: capture all four text-panel handles + the Mosaic output
    // container at bootstrap. The Reset button writes through these
    // handles (mapping/csvw/csv/shex) and clears the container —
    // re-querying the DOM at click time would re-couple the reset
    // module to the bootstrap layout.
    const mappingHandle = mountMappingPanel(host('panel-mapping'), mapping);
    const csvwHandle    = mountCsvwPanel(host('panel-csvw'), csvw);
    const csvHandle     = mountCsvPanel(host('panel-csv'), csv);
    const shexHandle    = mountShexPanel(host('panel-shex'), shex);
    mountOutputPanel(host('panel-output'), '');
    const outputContainer = document.querySelector<HTMLElement>(
        '#panel-output .mosaic-target',
    );
    if (!outputContainer) {
        throw new Error(
            '#panel-output .mosaic-target missing — output panel mount failed',
        );
    }

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
            // 07-08: load-bearing 10MB CSV refusal — happens BEFORE we
            // ever touch the LSP Worker or DuckDB Worker. The CSV-panel
            // paste guard is belt-and-suspenders; this is what enforces
            // the SC#4 cap across every code path (paste, drop, reset,
            // programmatic, future URL-state-loaded mappings).
            try {
                assertWithinLimit(csvHandle.text);
            } catch (e) {
                if (e instanceof CsvTooLargeError) {
                    await showCsvLimitModal(e);
                    return;                  // skip compile + DuckDB altogether
                }
                throw e;
            }

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

    // 7. Reset button (07-08 / PLAY-12 / SC#4). resetPlayground()
    //    terminates the DuckDB-WASM Worker (memory reclaim per P-MOD-1),
    //    clears the Mosaic output, and reloads the default example into
    //    the four text panels. The fossil-wasm LSP Worker is NOT touched
    //    — long-lived editor session per ADR-0026.
    //
    //    The button is rendered disabled in index.html so we can't
    //    invoke the reset path before the panel handles exist; flip to
    //    enabled here (after all mounts succeeded) and toggle around
    //    each click to prevent double-trigger during the in-flight
    //    `fetchTextOrEmpty` calls.
    const resetBtn = document.getElementById('btn-reset') as HTMLButtonElement | null;
    if (!resetBtn) {
        throw new Error('Reset button (#btn-reset) missing from index.html');
    }
    resetBtn.disabled = false;
    resetBtn.addEventListener('click', async () => {
        resetBtn.disabled = true;
        runBtn.disabled   = true;       // can't Run while panels are being rewritten
        try {
            await resetPlayground({
                mappingHandle, csvwHandle, csvHandle, shexHandle,
                outputContainer,
            });
        } catch (err) {
            console.error('reset failed:', err);
            alert(`Reset failed: ${String(err)}`);
        } finally {
            resetBtn.disabled = false;
            runBtn.disabled   = false;
        }
    });

    console.info('playground: panels + LSP + DuckDB Run + Reset wired (07-08)');
}

bootstrap().catch((err: unknown) => {
    console.error('playground bootstrap failed:', err);
    const msg = err instanceof Error ? err.message : String(err);
    document.body.insertAdjacentHTML(
        'beforeend',
        `<pre style="color:#a00;padding:1rem;">Bootstrap error: ${msg}</pre>`,
    );
});
