// Phase 1 WASM smoke test (Plan 01-09 / RESEARCH.md Example 18).
//
// Run after:
//   cargo build --release --target wasm32-unknown-unknown -p fossil-wasm
//   wasm-bindgen target/wasm32-unknown-unknown/release/fossil_wasm.wasm \
//     --out-dir crates/fossil-wasm/pkg --target nodejs
//
// Asserts that compile() returns SQL containing COPY and 'output.parquet'
// and a manifest mentioning graphar_version (Phase 1 success criterion #3).
//
// NOTE: this is a DIFFERENT bindgen target (`--target nodejs`, CommonJS-
// friendly with `require('fs')`-based .wasm loading) than playground-poc/
// from Phase 0 (`--target web`, which uses the browser fetch+instantiate
// pattern). Both are produced from the same fossil_wasm.wasm binary; the
// generated JS shim differs.

'use strict';

const fs = require('node:fs');
const path = require('node:path');

const { FossilPlayground } = require('./pkg/fossil_wasm.js');

function fail(msg) {
    console.error(`FAIL: ${msg}`);
    process.exit(1);
}

function main() {
    // Resolve the fixture relative to this script. __dirname here is
    // crates/fossil-wasm; the fixture lives at <repo-root>/examples/hello.fossil.
    // No hardcoded absolute paths per the project-wide rule.
    const sourcePath = path.resolve(__dirname, '..', '..', 'examples', 'hello.fossil');
    if (!fs.existsSync(sourcePath)) {
        fail(`fixture missing: ${sourcePath}`);
    }
    const source = fs.readFileSync(sourcePath, 'utf8');

    const pg = new FossilPlayground();
    let result;
    try {
        result = pg.compile(source);
    } catch (e) {
        fail(`compile() threw: ${e}`);
    }

    console.log('--- compile() returned ---');
    console.log(JSON.stringify(result, null, 2));

    if (!result || typeof result !== 'object') fail('result is not an object');
    if (typeof result.sql !== 'string') fail('result.sql is missing or not a string');
    if (!result.sql.includes('COPY')) fail("result.sql does not contain 'COPY'");
    if (!result.sql.includes('output.parquet')) {
        fail("result.sql does not contain 'output.parquet'");
    }
    if (typeof result.manifest_yaml !== 'string') {
        fail('result.manifest_yaml is missing or not a string');
    }
    if (!result.manifest_yaml.includes('graphar_version')) {
        fail("result.manifest_yaml missing 'graphar_version'");
    }

    console.log('OK: WASM smoke test passed');
}

main();
