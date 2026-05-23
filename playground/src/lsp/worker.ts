// The fossil-wasm LSP Worker. Long-lived (one per editor session).
// Vite bundles this file as a separate Worker chunk; the main thread
// spawns it via `new Worker(new URL('./worker.ts', import.meta.url),
// { type: 'module' })`.
//
// 07-03 ships the Rust `start_lsp_worker()` entrypoint inside the
// fossil-wasm crate; it installs the global `onmessage` handler that
// drives the postMessage JSON-RPC bridge.

import init, { start_lsp_worker } from '../../pkg-lsp-worker/fossil_wasm.js';

async function bootstrap(): Promise<void> {
    await init();             // wasm-bindgen --target web loader
    start_lsp_worker();       // installs the onmessage handler (07-03)
}

bootstrap().catch((e: unknown) => {
    // Worker-side errors don't bubble naturally; log to the Worker's own
    // console. monaco-languageclient ignores unknown postMessage shapes,
    // so sentinel posting is unnecessary here.
    console.error('worker bootstrap failed:', e);
});
