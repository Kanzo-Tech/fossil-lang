// Node-side smoke for playground/src/viz/mosaic.ts (W-mosaic fix).
//
// Catches vgplot API surface drift (tableMark / table / from / literal)
// + general render-path regressions BEFORE the much heavier Playwright
// run in 07-10. A node-test on a synthetic 3-row Arrow table runs in
// seconds; Playwright is minutes.
//
// 07-07 Task 1 took the documented plain-`<table>` fallback after
// probing @uwdata/vgplot 0.10 — these tests now mechanically pin the
// fallback contract: renderVertexEdge MUST produce a `<section.vx-table>`
// per non-empty arrow table + an `.empty-graph` note on the all-empty
// path. If a future vgplot bump rewires `mosaic.ts` to the real vgplot
// path (without changing the public DOM contract), these tests keep
// passing; if the rewire breaks the DOM contract, the assertions fail
// LOUD and the 07-08 reset wiring + 07-10 Playwright spec still hold.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import * as Arrow from 'apache-arrow';

function installDomGlobals() {
    const dom = new JSDOM('<!doctype html><div id="target"></div>');
    global.document = dom.window.document;
    global.window = dom.window;
    global.HTMLElement = dom.window.HTMLElement;
    global.Node = dom.window.Node;
    global.CustomEvent = dom.window.CustomEvent;
    return dom;
}

test('renderVertexEdge renders vertex+edge sections on synthetic 3-row tables', async () => {
    installDomGlobals();
    const { renderVertexEdge } = await import('../../src/viz/mosaic.ts');

    const vertices = Arrow.tableFromArrays({
        id:    ['v1', 'v2', 'v3'],
        label: ['User', 'User', 'User'],
    });
    const edges = Arrow.tableFromArrays({
        src_id: ['v1', 'v2', 'v3'],
        dst_id: ['v2', 'v3', 'v1'],
        type:   ['knows', 'knows', 'knows'],
    });

    const target = document.getElementById('target');
    assert.doesNotThrow(() => renderVertexEdge(target, vertices, edges));

    // Plain-<table> fallback contract: two `<section.vx-table>` children
    // (one for vertices, one for edges).
    const sections = target.querySelectorAll('section.vx-table');
    assert.equal(sections.length, 2, 'expected 2 vx-table sections (vertices + edges)');

    // Vertices section: heading text + 3 body rows + 2 header cells.
    const vertexSection = sections[0];
    assert.match(vertexSection.querySelector('h3').textContent, /Vertices \(3\)/);
    assert.equal(vertexSection.querySelectorAll('tbody tr').length, 3);
    assert.equal(vertexSection.querySelectorAll('thead th').length, 2);

    // Edges section: heading + 3 rows + 3 header cells.
    const edgeSection = sections[1];
    assert.match(edgeSection.querySelector('h3').textContent, /Edges \(3\)/);
    assert.equal(edgeSection.querySelectorAll('tbody tr').length, 3);
    assert.equal(edgeSection.querySelectorAll('thead th').length, 3);
});

test('renderVertexEdge handles the all-empty case with a "no triples" note', async () => {
    installDomGlobals();
    const { renderVertexEdge } = await import('../../src/viz/mosaic.ts');

    const empty = Arrow.tableFromArrays({ id: [] });
    const target = document.getElementById('target');
    renderVertexEdge(target, empty, empty);

    assert.match(target.textContent ?? '', /no triples/i);
    assert.equal(target.querySelector('p.empty-graph'),
                 target.firstChild,
                 'expected a single .empty-graph note');
});

test('renderVertexEdge clears prior render on re-call (re-run hygiene)', async () => {
    installDomGlobals();
    const { renderVertexEdge } = await import('../../src/viz/mosaic.ts');

    const v1 = Arrow.tableFromArrays({ id: ['a'] });
    const v2 = Arrow.tableFromArrays({ id: ['b', 'c'] });
    const e0 = Arrow.tableFromArrays({ src: [], dst: [] });

    const target = document.getElementById('target');
    renderVertexEdge(target, v1, e0);
    const firstRender = target.innerHTML;
    renderVertexEdge(target, v2, e0);
    const secondRender = target.innerHTML;

    assert.notEqual(firstRender, secondRender,
        'second render must replace first (target.replaceChildren on entry)');
    // Confirm the v2 row count (2) is reflected in the new heading.
    assert.match(target.textContent, /Vertices \(2\)/);
    assert.doesNotMatch(target.textContent, /Vertices \(1\)/);
});
