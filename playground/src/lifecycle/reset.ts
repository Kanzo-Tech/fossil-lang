// Playground reset path (PLAY-12 / Phase-7 SC#4).
//
// `resetPlayground()` is what the "Reset playground" button calls. It:
//   1. terminate()s the DuckDB-WASM Worker — the ONLY reliable reclaim
//      of its ~6.4MB heap (P-MOD-1). The fossil-wasm LSP Worker is NOT
//      touched; ADR-0026 documents the two-Worker / two-lifecycle
//      decision.
//   2. Clears the Mosaic output container — visual-side hygiene; the
//      DuckDB-Worker reset above does the memory-side work.
//   3. Reloads the default example into the four text panels (mapping,
//      CSVW, CSV, ShEx). After this returns, the next Run lazy-recreates
//      the DuckDB Worker from scratch (per duckdb/runner.ts singletons).
//
// Intentionally NOT symmetric: there is no `resetLsp()` here. The LSP
// Worker is long-lived; resetting it would lose the editor session
// (models, cursor, undo stack) — see ADR-0026 §"Decision".

import { resetDuckDb } from '../duckdb/lifecycle';

/**
 * Minimal surface the reset path needs from each text panel — just the
 * `text` setter (and a getter so future tests can read state back).
 * Matches PanelHandle's shape so the existing mount*Panel return values
 * satisfy this without an adapter.
 */
export interface TextSlot {
    get text(): string;
    set text(v: string);
}

/** Everything `resetPlayground` writes to when restoring the default example. */
export interface ResetTargets {
    mappingHandle: TextSlot;
    csvwHandle:    TextSlot;
    csvHandle:     TextSlot;
    shexHandle:    TextSlot;
    /** The `.mosaic-target` element inside #panel-output. */
    outputContainer: HTMLElement;
}

async function fetchTextOrEmpty(url: string): Promise<string> {
    try {
        const r = await fetch(url);
        if (!r.ok) return '';
        return await r.text();
    } catch {
        return '';
    }
}

/**
 * Full playground reset — see file header for what runs and in what
 * order. Idempotent: safe to call before any Run (no-op terminate when
 * the Worker hasn't been spawned yet).
 *
 * The function is `async` purely because the example fetch is async;
 * the DuckDB terminate + Mosaic clear are synchronous.
 */
export async function resetPlayground(targets: ResetTargets): Promise<void> {
    // 1. Memory reclaim — terminate the DuckDB-WASM Worker. The next
    //    runCompiledSql() call will re-run the lazy-import +
    //    instantiate path (browser cache typically keeps the 6.4MB
    //    payload around, so cold-start is fast on subsequent Resets).
    resetDuckDb();

    // 2. Visual reclaim — wipe the rendered vertex/edge tables. We
    //    leave the panel alive (its CustomEvent listener is still
    //    valid); only the children go.
    targets.outputContainer.replaceChildren();

    // 3. Reload the default example into all four text panels. Done
    //    in parallel because the four examples are independent files.
    //    `.fossil` and `.csv` are required (the mapping needs both);
    //    `.csvw.json` and `.shex` may be absent in some example sets.
    const [mapping, csvw, csv, shex] = await Promise.all([
        fetchTextOrEmpty('/examples/hello.fossil'),
        fetchTextOrEmpty('/examples/hello.csvw.json'),
        fetchTextOrEmpty('/examples/hello.csv'),
        fetchTextOrEmpty('/examples/hello.shex'),
    ]);

    targets.mappingHandle.text = mapping;
    targets.csvwHandle.text    = csvw;
    targets.csvHandle.text     = csv;
    targets.shexHandle.text    = shex;

    // TODO(PLAY-05 / Phase 8): when the example-picker lands, wire
    // resetPlayground to also fire on "switch example" — the same
    // memory-reclaim story applies as for the manual Reset button.
}
