// WASM Workspace lifecycle smoke.
//
// Verifies the ty_wasm-shaped lifecycle
// (openFile / updateFile / closeFile / diagnostics)
// round-trips through wasm-bindgen + serde-wasm-bindgen
// correctly in a node process. The native cargo-test mirror is
// `crates/fossil-wasm/tests/workspace.rs` (catches API regressions on every
// PR without needing the wasm-bindgen toolchain); this script is the
// JS-side rehearsal that proves wasm-bindgen serialization actually works
// in node ≥18: the workspace surface has to be testable from node, not only
// from a browser.
//
// Build precondition:
//   cargo build --release --target wasm32-unknown-unknown -p fossil-wasm
//   wasm-bindgen target/wasm32-unknown-unknown/release/fossil_wasm.wasm \
//     --out-dir crates/fossil-wasm/pkg --target nodejs
//
// Run:
//   node crates/fossil-wasm/test-wasm-workspace.js
//
// NOTE: this is the `--target nodejs` bindgen target (CommonJS-friendly,
// `require('fs')`-based .wasm loading) — distinct from the `--target web`
// artefact the browser packages consume. Both come from the same .wasm binary;
// only the generated JS shim differs.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

const { FossilWorkspace } = require('./pkg/fossil_wasm.js');

function fail(msg) {
    console.error(`FAIL: ${msg}`);
    process.exit(1);
}

function readHello() {
    // __dirname here is crates/fossil-wasm; the fixture lives at <repo-root>/examples/hello.fossil.
    const p = path.resolve(__dirname, '..', '..', 'examples', 'hello.fossil');
    if (!fs.existsSync(p)) fail(`fixture missing: ${p}`);
    return fs.readFileSync(p, 'utf8');
}

function main() {
    const ws = new FossilWorkspace();
    const source = readHello();

    // ----- openFile -----
    const h1 = ws.openFile('a.fossil', source);
    if (h1 === undefined || h1 === null) fail('openFile returned no FileHandle');
    console.log('openFile(a.fossil) ->', h1);

    // ----- updateFile -----
    ws.updateFile(h1, source + '\n// edit');  // must not throw

    // ----- diagnostics -----
    const diags1 = ws.diagnostics();
    assert.ok(Array.isArray(diags1), 'diagnostics returns an array');
    console.log(`diagnostics() -> ${diags1.length} rows`);
    // Every row carries the Diagnostic shape — { uri, range, severity, code, message, title, data, … }.
    // That is a diagnostics panel's shape, NOT the LSP wire: the worker
    // publishes lsp_types::Diagnostic, which has no `uri` field.
    for (const d of diags1) {
        assert.ok(typeof d.uri === 'string', 'row.uri is a string');
        assert.ok(d.range && d.range.start && d.range.end, 'row.range is well-formed');
        assert.ok(typeof d.range.start.line === 'number', 'row.range.start.line is a number');
        assert.ok(typeof d.severity === 'number', 'row.severity is a number');
        assert.ok(typeof d.message === 'string', 'row.message is a string');
        assert.ok(typeof d.code === 'string' && d.code.includes('/'), 'row.code is area/kind');
        assert.ok(typeof d.title === 'string', 'row.title is a string');
        assert.ok(d.data !== null && typeof d.data === 'object' && !(d.data instanceof Map), 'row.data is a plain object');
    }

    // ----- multi-file isolation -----
    ws.openFile('b.fossil', source);
    const both = ws.diagnostics();
    // Every row carries the URI of the file it is about: one check answers every open file.
    for (const d of both) assert.ok(d.uri === 'a.fossil' || d.uri === 'b.fossil', `row.uri ${d.uri}`);

    // ----- closeFile -----
    ws.closeFile(h1);
    // Closing the same handle again must throw — strict signal mirrors ty_wasm.
    let threwClose = false;
    try { ws.closeFile(h1); } catch (_e) { threwClose = true; }
    assert.ok(threwClose, 'closeFile of closed handle throws');

    // updateFile on a closed handle must also throw.
    let threwUpdate = false;
    try { ws.updateFile(h1, '// post-close'); } catch (_e) { threwUpdate = true; }
    assert.ok(threwUpdate, 'updateFile of closed handle throws');

    // The other file remains usable.
    const diagsAfterClose = ws.diagnostics();
    assert.ok(Array.isArray(diagsAfterClose), 'b.fossil still checkable after closing h1');
    for (const d of diagsAfterClose) assert.equal(d.uri, 'b.fossil');

    console.log('OK: workspace lifecycle smoke passed');
}

main();
