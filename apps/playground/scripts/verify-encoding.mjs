/**
 * Check what the app draws the corpus WITH against what the corpus says — in Node, no browser.
 *
 * `src/encoding.ts` replaced three string literals in `src/Canvas.tsx` with a derivation, and a
 * derivation is only worth the literals it replaced if it can be run against a real artefact and
 * held to an answer. That is this script: it opens the bench corpus through the same door the tab
 * opens, computes the same `Encoding` the component computes, and checks every field of it against
 * the manifest and the bytes independently — never against a value written down here.
 *
 * What it asserts:
 *
 *   1. **The colour is a column the payload carries and the manifest does not declare.** This is
 *      the whole reason the derivation reads `Corpus.types` rather than the vertex manifest's
 *      `properties:` — the layout pass's output is in the bytes and in no `properties:` entry, so a
 *      reader that believed the manifest would find no column to colour a corpus that has one.
 *   2. **The colour is what the DOOR would have used.** The same rectangle asked twice — once
 *      naming the derived column, once naming nothing — comes back with identical `categories`. If
 *      `FrameParams.fill`'s default and this app's derivation ever name different columns, one of
 *      the two pictures is wrong and this is what says so.
 *   3. **The chart's column is declared, numeric, and none of the four it may not be** — not the
 *      address, not a coordinate, not the identity, not the colour.
 *   4. **Every column the crossfilter view projects exists in the payload.** A `CREATE VIEW` over a
 *      column that is not there fails at open, in a browser, with the corpus already fetched.
 *   5. **The label is the manifest's primary property**, reached through `Corpus.types.identity`
 *      rather than by reading `is_primary` — two paths to one answer, which is the check.
 *   6. **The fallbacks answer.** With the declaration withheld the encoding still resolves a chart
 *      column, because a corpus with no `privacy:` block is a legal corpus and the commonest kind.
 *
 * Run: `node --experimental-strip-types scripts/verify-encoding.mjs [--vertices 1000000]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { openCorpus } from '@fossil-lang/corpus';

import { query as duckQuery } from '../../corpus/guards/duck.mjs';

/** The reader's wasm as BYTES — same reason as `verify-canvas.mjs`: undici refuses `file:`. */
const CORPUS_WASM = new Response(
  await readFile(new URL('../node_modules/@fossil-lang/corpus/pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
  { headers: { 'content-type': 'application/wasm' } },
);

/** `./x.js` from a `.ts` file, resolved to the `.ts`. Registered before the dynamic import below. */
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith('.') && specifier.endsWith('.js') && context.parentURL) {
      const asJs = new URL(specifier, context.parentURL);
      const asTs = new URL(`${specifier.slice(0, -3)}.ts`, context.parentURL);
      if (!existsSync(fileURLToPath(asJs)) && existsSync(fileURLToPath(asTs))) {
        return nextResolve(`${specifier.slice(0, -3)}.ts`, context);
      }
    }
    return nextResolve(specifier, context);
  },
});

const { encodingFor } = await import('../src/encoding.ts');

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
};

const stampPath = resolve(app, 'public/bench/index.json');
if (!existsSync(stampPath)) {
  console.error('no bench corpus — run `node scripts/bench-corpus.mjs` first');
  process.exit(2);
}
const stamp = JSON.parse(readFileSync(stampPath, 'utf8'));
const count = Number(flag('vertices', stamp.count));
const root = resolve(app, 'public/bench', String(count));
if (!existsSync(join(root, 'graph.graph.yml'))) {
  console.error(`no corpus at ${root} — run \`node scripts/bench-corpus.mjs --vertices ${count}\``);
  process.exit(2);
}

let failures = 0;
const ok = (label, condition, detail = '') => {
  if (condition) console.log(`  ok   ${label}${detail ? ` — ${detail}` : ''}`);
  else {
    console.log(`  FAIL ${label}${detail ? ` — ${detail}` : ''}`);
    failures++;
  }
};

/**
 * The manifest, read by hand — **a third parser, deliberately, and that is the point of it.**
 *
 * `packages/corpus/src/manifest.ts` and `apps/corpus/guards/manifest.mjs` are the other two, and
 * `src/bound.ts` is the app's reader of the `privacy:` block. This script must not import that one:
 * it reaches `src/duckdb.js`, which imports a browser wasm bundle through a Vite `?url` specifier
 * and does not resolve in Node. So the two facts this check needs are read off the text here, in
 * the flat two-space shape `crates/fossil-sinks` asserts precisely so that independent parsers
 * agree. An encoding checked against a value this file computed a second way is a check; one
 * checked against a value the app handed it is a tautology.
 */
const indexText = readFileSync(join(root, 'graph.graph.yml'), 'utf8');
const quasiIdentifiers = (/^ {2}quasi_identifiers:\s*(.+)$/m.exec(indexText)?.[1] ?? '')
  .trim()
  .split(/\s+/)
  .filter((q) => q !== '');

const manifestPaths = [];
{
  let inList = false;
  for (const raw of indexText.split('\n')) {
    if (/^(vertices|edges):\s*$/.test(raw)) {
      inList = true;
      continue;
    }
    const item = /^-\s+(\S+)\s*$/.exec(raw);
    if (inList && item) manifestPaths.push(item[1]);
    else if (!/^\s*$/.test(raw) && !item) inList = false;
  }
}

/**
 * The PAYLOAD projection's declared properties — `scale: 1`, which is the first `projections:`
 * entry and the only one that declares any. Name and `is_primary`, which is all a property is.
 */
function declaredProperties(text) {
  const out = [];
  let inPayload = false;
  let current = null;
  for (const raw of text.split('\n')) {
    if (/^projections:\s*$/.test(raw)) continue;
    const entry = /^- path: (.*)$/.exec(raw);
    if (entry) {
      // `path: ''` is the payload; `path: l1/` and its siblings are the levels, which declare none.
      inPayload = entry[1].trim() === "''" || entry[1].trim() === '';
      continue;
    }
    if (!inPayload) continue;
    const name = /^ {2}- name: (.+)$/.exec(raw);
    if (name) {
      current = { name: name[1].trim(), primary: false };
      out.push(current);
      continue;
    }
    const primary = /^ {4}is_primary: (.+)$/.exec(raw);
    if (primary && current !== null) current.primary = primary[1].trim() === 'true';
  }
  return out;
}

const query = async (sql) => duckQuery(sql);

const corpus = await openCorpus(root, { query, wasmUrl: CORPUS_WASM });
const declaredType = corpus.types.vertices[0];
const properties = declaredProperties(readFileSync(join(root, manifestPaths[0]), 'utf8'));

console.log(
  `corpus: ${count} vertices · ${declaredType.fields.length} columns on disk · ` +
    `${properties.length} declared properties · quasi-identifiers: ${quasiIdentifiers.join(' ') || 'none'}`,
);

const encoding = encodingFor({ types: corpus.types, quasiIdentifiers });
console.log(
  `encoding: type=${encoding.type} fill=${encoding.fill} brush=${encoding.brush} ` +
    `label=${encoding.label} columns=[${encoding.columns.join(', ')}]`,
);

const carried = new Set(declaredType.fields.map((f) => f.name));
const declaredNames = new Set(properties.map((p) => p.name));
const typeOf = (name) => declaredType.fields.find((f) => f.name === name)?.type ?? null;
const NUMERIC = /^(U?TINYINT|U?SMALLINT|U?INTEGER|U?BIGINT|U?HUGEINT|FLOAT|DOUBLE|REAL|DECIMAL)/;

console.log('\n1 · the colour comes from the bytes, because the manifest has nowhere to say it');
ok('a colour was derived', encoding.fill !== null, String(encoding.fill));
ok('the payload carries it', carried.has(encoding.fill), typeOf(encoding.fill) ?? 'absent');
ok(
  'and no `properties:` entry declares it',
  !declaredNames.has(encoding.fill),
  `declared: ${[...declaredNames].join(', ')}`,
);
ok('it is an integral column, so an ordinal can be read out of it', /^U?(TINY|SMALL|)INT|^U?INTEGER|^U?BIGINT|^U?HUGEINT/.test(typeOf(encoding.fill) ?? ''), typeOf(encoding.fill) ?? '');

console.log('\n2 · the colour is the column the door would have used anyway');
const extent = await corpus.extent();
/**
 * The middle half of the corpus, at a resolution that draws thousands rather than a handful.
 *
 * Both halves matter. A rectangle in a CORNER of the extent can be empty — the layout places
 * communities, not a uniform field — and an equality over two empty answers passes without
 * comparing anything. And the level is derived from `pixels`, so a small canvas over a large
 * rectangle decimates to almost nothing: this asked a corner tenth at 128×128 and compared **one**
 * mark, which is a green tick over a check that was not performed.
 */
const box = {
  x: extent.minX + (extent.maxX - extent.minX) / 4,
  y: extent.minY + (extent.maxY - extent.minY) / 4,
  w: (extent.maxX - extent.minX) / 2,
  h: (extent.maxY - extent.minY) / 2,
};
const pixels = { w: 512, h: 512 };
const named = await corpus.frame({ ...box, pixels, fill: encoding.fill });
const defaulted = await corpus.frame({ ...box, pixels });
const sameCategories =
  named.categories.length === defaulted.categories.length &&
  named.categories.every((v, i) => v === defaulted.categories[i]);
ok('the frame has marks to compare', named.marks > 1000, `${named.marks} marks`);
ok(
  'naming the derived column and naming nothing answer the same categories',
  sameCategories,
  `${named.categories.length} rows`,
);

console.log('\n3 · the chart brushes a declared, numeric, non-positional column');
ok('a column was derived', encoding.brush !== null, String(encoding.brush));
ok('the payload carries it', carried.has(encoding.brush), typeOf(encoding.brush) ?? 'absent');
ok('it is numeric', NUMERIC.test(typeOf(encoding.brush) ?? ''), typeOf(encoding.brush) ?? '');
ok(
  'the manifest declares it a quasi-identifier',
  quasiIdentifiers.includes(`${encoding.type}.${encoding.brush}`),
  quasiIdentifiers.join(' ') || 'none declared',
);
ok('it is not the address', encoding.brush !== 'dense_id');
ok('it is not a coordinate', encoding.brush !== 'x' && encoding.brush !== 'y');
ok('it is not the identity', encoding.brush !== encoding.label);
ok('it is not the colour — brushing that would be brushing the legend', encoding.brush !== encoding.fill);

console.log("\n4 · every column the crossfilter's view projects is one the payload has");
for (const column of encoding.columns) {
  ok(`\`${column}\` is on disk`, carried.has(column), typeOf(column) ?? 'absent');
}
ok('the address is among them — the view is keyed by it', encoding.columns.includes('dense_id'));
ok(
  'the identity is NOT — it is the widest column and nothing filters by it',
  !encoding.columns.includes(encoding.label),
  encoding.label ?? 'none',
);

console.log('\n5 · the label is the manifest’s primary property, reached by the other route');
const primary = properties.find((p) => p.primary)?.name ?? null;
ok('the manifest declares a primary property', primary !== null, String(primary));
ok('and `Corpus.types` calls the same column the identity', encoding.label === primary, `${encoding.label} / ${primary}`);

console.log('\n6 · a corpus that declares nothing still gets an encoding');
const undeclared = encodingFor({ types: corpus.types, quasiIdentifiers: [] });
ok('the colour is unchanged — it never came from the declaration', undeclared.fill === encoding.fill);
ok('a chart column is still derived', undeclared.brush !== null, String(undeclared.brush));
ok(
  'the projection drops the quasi-identifiers and keeps the address and the colour',
  undeclared.columns.join(',') === ['dense_id', encoding.fill].join(','),
  `[${undeclared.columns.join(', ')}]`,
);

console.log(`\n${failures === 0 ? 'all checks passed' : `${failures} FAILED`}`);
process.exit(failures === 0 ? 0 : 1);
