// Playground entrypoint — fetches the baked example files and mounts the
// five panel placeholders. Subsequent plans replace these placeholders:
//
//   07-05  → Monaco editors + monaco-languageclient (mapping panel)
//   07-06  → DuckDB-WASM lazy-load + Run wiring
//   07-07  → Mosaic / @uwdata/vgplot in the output panel
//   07-08  → Reset button + URL-based state sharing
//
// The example fetch URLs resolve against `public/examples/*` which Vite
// serves at the site root (`/examples/...`) in dev and copies into
// `dist/examples/` at build time.

import { mountMappingPanel } from './panels/mapping-panel';
import { mountCsvwPanel }    from './panels/csvw-panel';
import { mountCsvPanel }     from './panels/csv-panel';
import { mountShexPanel }    from './panels/shex-panel';
import { mountOutputPanel }  from './panels/output-panel';

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
    // Load default-example texts (research §"Open Question 5" — recommended:
    // ship the walking-skeleton hello.fossil as the default content).
    const [mapping, csvw, csv, shex] = await Promise.all([
        fetchText('/examples/hello.fossil',   ''),
        fetchText('/examples/hello.csvw.json', ''),
        fetchText('/examples/hello.csv',       ''),
        fetchText('/examples/hello.shex',      ''),
    ]);

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

    // Wire-up checkpoint — emits one console line at bootstrap completion.
    // Later plans (07-05/06/07/08) append their own bootstrap lines from
    // here. Plain console.info, not a logging library — keeps the v0.1
    // bundle lean (the SC#3 size budget is tight).
    console.info(
        'playground: panels mounted; LSP + DuckDB wiring lands in subsequent plans',
    );
}

bootstrap().catch((err: unknown) => {
    console.error('playground bootstrap failed:', err);
    const msg = err instanceof Error ? err.message : String(err);
    document.body.insertAdjacentHTML(
        'beforeend',
        `<pre style="color:#a00;padding:1rem;">Bootstrap error: ${msg}</pre>`,
    );
});
