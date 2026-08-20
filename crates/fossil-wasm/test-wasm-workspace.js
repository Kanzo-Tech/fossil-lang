// WASM Workspace lifecycle smoke.
//
// Companion to test-wasm.js — verifies the ty_wasm-shaped lifecycle
// (open_file / update_file / close_file / check / compile_file /
// diagnostics_for) round-trips through wasm-bindgen + serde-wasm-bindgen
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
// NOTE: this is the same `--target nodejs` bindgen target as `test-wasm.js`
// (CommonJS-friendly, `require('fs')`-based .wasm loading) — distinct from the
// `--target web` artefact the browser packages consume. Both come from the same
// .wasm binary; only the generated JS shim differs.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

const { FossilPlayground } = require('./pkg/fossil_wasm.js');

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
    const pg = new FossilPlayground();
    const source = readHello();

    // ----- open_file -----
    const h1 = pg.open_file('a.fossil', source);
    if (h1 === undefined || h1 === null) fail('open_file returned no FileHandle');
    console.log('open_file(a.fossil) ->', h1);

    // ----- update_file -----
    pg.update_file(h1, source + '\n// edit');  // must not throw

    // ----- check -----
    const diags1 = pg.check();
    assert.ok(Array.isArray(diags1), 'check returns an array');
    console.log(`check() -> ${diags1.length} rows`);
    // Every row carries the LSP shape — { uri, range, severity, message }.
    for (const d of diags1) {
        assert.ok(typeof d.uri === 'string', 'row.uri is a string');
        assert.ok(d.range && d.range.start && d.range.end, 'row.range is well-formed');
        assert.ok(typeof d.range.start.line === 'number', 'row.range.start.line is a number');
        assert.ok(typeof d.severity === 'number', 'row.severity is a number');
        assert.ok(typeof d.message === 'string', 'row.message is a string');
    }

    // ----- compile_file -----
    const out = pg.compile_file(h1);
    assert.ok(out && typeof out.sql === 'string', 'compile_file returns { sql, ... }');
    assert.ok(out.sql.includes('COPY'), 'compile_file SQL contains COPY');
    assert.ok(out.sql.includes('output.parquet'), 'compile_file SQL contains output.parquet');
    assert.ok(typeof out.manifest_yaml === 'string', 'compile_file returns { manifest_yaml }');
    assert.ok(out.manifest_yaml.includes('graphar_version'), 'manifest_yaml contains graphar_version');

    // ----- multi-file isolation + diagnostics_for (B3) -----
    const h2 = pg.open_file('b.fossil', source);
    const perFileA = pg.diagnostics_for(h1);
    const perFileB = pg.diagnostics_for(h2);
    assert.ok(Array.isArray(perFileA), 'diagnostics_for(h1) returns an array');
    assert.ok(Array.isArray(perFileB), 'diagnostics_for(h2) returns an array');
    // Every row drained for h1 carries h1's URI; same for h2 (per-file scoping).
    for (const d of perFileA) assert.equal(d.uri, 'a.fossil');
    for (const d of perFileB) assert.equal(d.uri, 'b.fossil');

    // ----- close_file -----
    pg.close_file(h1);
    // Closing the same handle again must throw — strict signal mirrors ty_wasm.
    let threwClose = false;
    try { pg.close_file(h1); } catch (_e) { threwClose = true; }
    assert.ok(threwClose, 'close_file of closed handle throws');

    // update_file on a closed handle must also throw.
    let threwUpdate = false;
    try { pg.update_file(h1, '// post-close'); } catch (_e) { threwUpdate = true; }
    assert.ok(threwUpdate, 'update_file of closed handle throws');

    // The other file remains usable.
    const diagsAfterClose = pg.diagnostics_for(h2);
    assert.ok(Array.isArray(diagsAfterClose), 'h2 still drainable after closing h1');

    console.log('OK: workspace lifecycle smoke passed');
}

main();
