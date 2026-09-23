/**
 * Check what the app draws the corpus WITH against what the corpus says — in Node, no browser.
 *
 * `@fossil-lang/draw` replaced three string literals in `src/Canvas.tsx` with a derivation, and a
 * derivation is only worth the literals it replaced if it can be run against a real artefact and
 * held to an answer. That is this script: it opens the bench corpus through the same door the tab
 * opens, computes the same `Encoding` the component computes, and checks every field of it against
 * the manifest and the bytes independently — never against a value written down here.
 *
 * What it asserts:
 *
 *   1. **The colour is what the corpus declares, or — where it declares nothing — what the bytes
 *      say.** A vertex type's `channels:` block names the column and the scale it is read on, and
 *      it is the only field in the format whose subject is the drawing. Where there is no such key
 *      the colour comes off `Corpus.types`, because the layout pass's output is in the bytes and in
 *      no `properties:` entry: a reader that believed the *property* list would find no column to
 *      colour a corpus that has one.
 *   2. **A column a corpus declares is a column the corpus has.** This replaced a comparison of
 *      `frame({fill})` against `frame({})`, which was a real cross-check while `encoding.ts` and
 *      `corpus.ts` wrote the name independently and is a tautology now that both read
 *      `PAYLOAD_CATEGORICAL`. What is worth checking instead is the declaration against the bytes:
 *      every channel's column is on disk, a `domain` appears exactly on a categorical, and the
 *      count it declares is the count a `count(DISTINCT …)` finds — the one number a reader cannot
 *      recover is also the one nothing else can catch being wrong. The door's default goes through
 *      the same door: a generated constant states what fossil's writer emits and cannot state what
 *      a given corpus carries.
 *   3. **The chart's column is numeric, none of the four it may not be** — not the address, not a
 *      coordinate, not the identity, not the colour — **and it comes from whichever declaration is
 *      making the claim**: a declared quantitative channel where there is one, the `privacy:`
 *      block's quasi-identifiers where there is not.
 *   4. **Every column the crossfilter view projects exists in the payload.** A `CREATE VIEW` over a
 *      column that is not there fails at open, in a browser, with the corpus already fetched.
 *   5. **The label is the manifest's primary property**, reached through `Corpus.types.identity`
 *      rather than by reading `is_primary` — two paths to one answer, which is the check.
 *   6. **The three states.** A corpus that declares nothing draws exactly as it drew before the
 *      block existed — checked against this file's own copy of the old rule, not against the app's
 *      answer — and a corpus that declares something is drawn by what it declares. This inverts
 *      what group 6 used to assert (*the colour never came from the declaration*), which is the
 *      invariant `/docs/design/position`'s `channels:` section retracts.
 *
 * Run: `node --experimental-strip-types scripts/verify-encoding.mjs [--vertices 1000000]`
 * Requires `scripts/bench-corpus.mjs` to have run, and the `duckdb` binary on PATH.
 */
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { open, PAYLOAD_CATEGORICAL } from '@fossil-lang/corpus';

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

const { encodingFor } = await import('@fossil-lang/draw');

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
 * `packages/corpus/src/manifest.ts` and `apps/corpus/guards/manifest.mjs` are two of the others,
 * and `src/bound.ts` is the app's reader of the `privacy:` block. This script must not import that
 * one: it reaches `src/duckdb.js`, which imports a browser wasm bundle through a Vite `?url`
 * specifier and does not resolve in Node. So the facts this check needs are read off the text here,
 * in the flat two-space shape `crates/fossil-sinks` asserts precisely so that independent parsers
 * agree. An encoding checked against a value this file computed a second way is a check; one
 * checked against a value the app handed it is a tautology.
 *
 * **`channels:` is read here too, and `@fossil-lang/draw`'s `readChannels` is deliberately not
 * imported** for that reason and no other. The app's scanner is what the tab runs; this one is what
 * holds it to an answer, and a harness that called the scanner it is checking would be asserting
 * that a function equals itself.
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

/**
 * The `channels:` block of one vertex manifest, in the three states the format has: `undefined` for
 * a document with no such key, `[]` for `channels: []`, and a list otherwise.
 *
 * Flat, because the writer's round-trip test freezes it flat: `- name:` opens an entry and two
 * spaces continue it, so an entry that nested would be invisible to half the readers of this
 * format. A `domain` is a number and appears only on a categorical; a `derived_by` appears exactly
 * when the writer computed the column.
 */
function declaredChannels(text) {
  const lines = text.split('\n');
  const at = lines.findIndex((l) => /^channels:\s*(\[\s*\])?\s*$/.test(l));
  if (at < 0) return undefined;
  if (lines[at].includes('[')) return [];
  const out = [];
  for (const raw of lines.slice(at + 1)) {
    const item = /^- ([a-z_]+):\s*(.*)$/.exec(raw);
    if (item) {
      out.push({ [item[1]]: item[2].trim() });
      continue;
    }
    const field = /^ {2}([a-z_]+):\s*(.*)$/.exec(raw);
    if (field && out.length > 0) {
      out[out.length - 1][field[1]] = field[2].trim();
      continue;
    }
    break;
  }
  return out.map((raw) => ({
    ...raw,
    ...(raw.domain === undefined ? {} : { domain: Number(raw.domain) }),
  }));
}

/**
 * The colour a corpus that declares nothing gets — **this file's own copy of the rule as it stood
 * before `channels:` existed**, and the whole point of it being here is that it is a copy.
 *
 * Group 6 asserts the app still answers this, which is what «a corpus written before the field
 * keeps drawing exactly as it drew» means operationally. Written out rather than imported for the
 * reason the manifest is scanned rather than imported.
 */
function colourBeforeChannels(type) {
  const drawable = type.fields.filter(
    (f) => f.name !== 'dense_id' && f.name !== 'x' && f.name !== 'y' && f.name !== type.identity,
  );
  const whole = (f) => /^U?(TINYINT|SMALLINT|INTEGER|BIGINT|HUGEINT)$/.test(f.type);
  const community = drawable.find((f) => f.name === 'cluster_id');
  if (community !== undefined && whole(community)) return community.name;
  return drawable.find(whole)?.name ?? null;
}

const query = async (sql) => duckQuery(sql);

const corpus = await open(root, { query, wasmUrl: CORPUS_WASM });
const declaredType = corpus.types.vertices[0];
const vertexManifest = readFileSync(join(root, manifestPaths[0]), 'utf8');
const properties = declaredProperties(vertexManifest);
const channels = declaredChannels(vertexManifest);

const stateOfBlock =
  channels === undefined ? 'no `channels:` key' : channels.length === 0 ? 'an empty list' : `${channels.length} declared`;

console.log(
  `corpus: ${count} vertices · ${declaredType.fields.length} columns on disk · ` +
    `${properties.length} declared properties · channels: ${stateOfBlock} · ` +
    `quasi-identifiers: ${quasiIdentifiers.join(' ') || 'none'}`,
);

const encoding = encodingFor({ types: corpus.types, channels, quasiIdentifiers });
console.log(
  `encoding: type=${encoding.type} fill=${encoding.fill} brush=${encoding.brush} ` +
    `label=${encoding.label} columns=[${encoding.columns.join(', ')}]`,
);

const carried = new Set(declaredType.fields.map((f) => f.name));
const declaredNames = new Set(properties.map((p) => p.name));
const typeOf = (name) => declaredType.fields.find((f) => f.name === name)?.type ?? null;
const NUMERIC = /^(U?TINYINT|U?SMALLINT|U?INTEGER|U?BIGINT|U?HUGEINT|FLOAT|DOUBLE|REAL|DECIMAL)/;

console.log('\n1 · the colour is what the corpus declares, or what the bytes say where it declares nothing');
const categoricals = (channels ?? []).filter((c) => c.scale === 'categorical');
ok('a colour was derived', encoding.fill !== null, String(encoding.fill));
ok('the payload carries it', carried.has(encoding.fill), typeOf(encoding.fill) ?? 'absent');
if (channels === undefined) {
  ok(
    'the manifest has no `channels:` key, so nothing was declared to read',
    true,
    'the colour is the bytes\u2019 answer',
  );
  ok(
    'and no `properties:` entry declares the column either — which is why the fallback reads the bytes',
    !declaredNames.has(encoding.fill),
    `declared: ${[...declaredNames].join(', ')}`,
  );
  ok(
    'it is an integral column, so an ordinal can be read out of it',
    /^U?(TINY|SMALL|)INT|^U?INTEGER|^U?BIGINT|^U?HUGEINT/.test(typeOf(encoding.fill) ?? ''),
    typeOf(encoding.fill) ?? '',
  );
} else {
  ok('the block declares a categorical channel', categoricals.length > 0, stateOfBlock);
  ok(
    'and the colour IS its column — nothing in the bytes outranks a declaration',
    encoding.fill === (categoricals[0]?.column ?? null),
    `${encoding.fill} / declared ${categoricals[0]?.column ?? 'none'}`,
  );
}

console.log('\n2 · a column a corpus declares is a column the corpus has');
for (const channel of channels ?? []) {
  ok(
    `\`${channel.name ?? '?'}\` reads \`${channel.column}\`, and the payload has it`,
    carried.has(channel.column),
    typeOf(channel.column) ?? 'absent',
  );
  ok(
    `\`${channel.name ?? '?'}\` is read on a scale a reader can dispatch on`,
    channel.scale === 'categorical' || channel.scale === 'quantitative',
    String(channel.scale),
  );
  ok(
    `\`${channel.name ?? '?'}\` carries a \`domain\` exactly if it is a categorical`,
    (channel.domain !== undefined) === (channel.scale === 'categorical'),
    `scale: ${channel.scale}, domain: ${channel.domain ?? 'absent'}`,
  );
  /**
   * The one number a reader cannot recover, recovered anyway — **which is the only way to find out
   * that it is wrong.**
   *
   * A distinct count is in no Parquet footer, which is why the field exists; it is in the COLUMN,
   * at the cost of a scan, and a verifier is exactly the place to pay that once. The tile URL comes
   * off the addressing rather than from a path spelled here, and under the `rowgroups` container
   * every tile is the same file, so tile 0's URL is the whole payload.
   */
  if (channel.scale === 'categorical' && channel.domain !== undefined && carried.has(channel.column)) {
    const payload = corpus.addressing.vertexType(declaredType.type).tileUrl(0);
    const rows = await duckQuery(
      `SELECT count(DISTINCT "${channel.column.replace(/"/g, '""')}") AS d FROM read_parquet('${payload}')`,
    );
    const measured = Number(rows[0]?.d ?? -1);
    ok(
      `\`${channel.name ?? '?'}\` declares the distinct count the bytes actually have`,
      measured === channel.domain,
      `declared ${channel.domain}, counted ${measured}`,
    );
  }
}
if (channels === undefined) {
  ok(
    'this corpus declares no channels, so the door falls back to a generated name',
    channels === undefined,
    'the three states are distinguishable: absent is not an empty list',
  );
}
/**
 * The door's default, checked against the artefact rather than against the app.
 *
 * `FrameParams.fill` defaults to `PAYLOAD_CATEGORICAL[0]` — a constant generated from `corpus.bnf`,
 * which states what fossil's writer emits and cannot state what THIS corpus has. Comparing it with
 * `encoding.fill` would be comparing two reads of one constant; comparing it with the columns on
 * disk is the check that was being stood in for.
 */
ok(
  'the door\u2019s default fill names a column this corpus carries',
  carried.has(PAYLOAD_CATEGORICAL[0]),
  `${PAYLOAD_CATEGORICAL[0]} — ${typeOf(PAYLOAD_CATEGORICAL[0]) ?? 'absent'}`,
);
const extent = await corpus.extent();
/**
 * The middle half of the corpus, at a resolution that draws thousands rather than a handful.
 *
 * Both halves matter. A rectangle in a CORNER of the extent can be empty — the layout places
 * communities, not a uniform field — and a frame of nothing passes every assertion about itself.
 * And the level is derived from `pixels`, so a small canvas over a large rectangle decimates to
 * almost nothing: this asked a corner tenth at 128×128 and drew **one** mark, which is a green tick
 * over a check that was not performed.
 */
const box = {
  x: extent.minX + (extent.maxX - extent.minX) / 4,
  y: extent.minY + (extent.maxY - extent.minY) / 4,
  w: (extent.maxX - extent.minX) / 2,
  h: (extent.maxY - extent.minY) / 2,
};
const pixels = { w: 512, h: 512 };
const named = await corpus.frame({ ...box, pixels, fill: encoding.fill });
ok('the frame has marks to draw', named.marks > 1000, `${named.marks} marks`);
ok(
  'and a category per mark, so the colour column answered',
  named.categories.length >= named.marks,
  `${named.categories.length} rows for ${named.marks} marks`,
);

console.log('\n3 · the chart brushes a declared, numeric, non-positional column');
const quantitatives = (channels ?? []).filter((c) => c.scale === 'quantitative');
ok('a column was derived', encoding.brush !== null, String(encoding.brush));
ok('the payload carries it', carried.has(encoding.brush), typeOf(encoding.brush) ?? 'absent');
ok('it is numeric', NUMERIC.test(typeOf(encoding.brush) ?? ''), typeOf(encoding.brush) ?? '');
// Which declaration is making the claim. A quantitative channel is a statement ABOUT the drawing
// and outranks `quasi_identifiers`, which is a disclosure-control field and was answering this
// question only because it was the only writer-side statement in reach.
if (quantitatives.length > 0) {
  ok(
    'the block declares a quantitative channel, and the chart is its column',
    encoding.brush === quantitatives[0].column,
    `${encoding.brush} / declared ${quantitatives[0].column}`,
  );
} else {
  ok(
    'no quantitative channel is declared, so the `privacy:` block still ranks — and it named this one',
    quasiIdentifiers.includes(`${encoding.type}.${encoding.brush}`),
    quasiIdentifiers.join(' ') || 'none declared',
  );
}
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

console.log('\n6 · the three states: what declares nothing draws as it did, what declares something is drawn by it');

/**
 * **A corpus that declares nothing draws exactly as it did.**
 *
 * Neither block — no `channels:` and no quasi-identifiers — which is the commonest legal corpus and
 * every corpus written before either field existed. The colour is checked against
 * `colourBeforeChannels`, this file's own copy of the rule as it stood, so what passes here is
 * «unchanged», not «whatever the module now returns».
 */
const undeclared = encodingFor({ types: corpus.types, quasiIdentifiers: [] });
const before = colourBeforeChannels(declaredType);
ok(
  'with no declaration at all, the colour is the pre-`channels:` rule’s answer',
  undeclared.fill === before,
  `${undeclared.fill} / ${before}`,
);
ok('a chart column is still derived', undeclared.brush !== null, String(undeclared.brush));
ok(
  'the projection drops the quasi-identifiers and keeps the address, the colour and the chart',
  undeclared.columns.join(',') ===
    ['dense_id', undeclared.fill, undeclared.brush].filter((c) => c !== null).join(','),
  `[${undeclared.columns.join(', ')}]`,
);

/**
 * **A corpus that declares something is drawn by what it declares** — and the declarations below
 * are SYNTHETIC, which the header of this script would otherwise forbid.
 *
 * They are legal because they are not the corpus's claim about itself being checked against itself:
 * groups 1 to 3 hold the app to the artefact's own block, and this half holds it to the rule in the
 * states no single artefact can be in at once. A corpus cannot simultaneously declare nothing,
 * declare an empty list, and declare a column its own writer would never choose — and «declared
 * wins» is only a check if the declared column differs from the derived one.
 */
const drawable = declaredType.fields.filter(
  (f) => f.name !== 'dense_id' && f.name !== 'x' && f.name !== 'y' && f.name !== declaredType.identity,
);
// A column the derivation would NOT have chosen, so that «declared wins» is a difference and not a
// coincidence — and a non-numeric one for preference, to leave both numeric columns brushable.
const asCategorical =
  (drawable.find((f) => !NUMERIC.test(f.type)) ?? drawable.find((f) => f.name !== before))?.name ?? null;
const brushCandidates = drawable.filter((f) => NUMERIC.test(f.type) && f.name !== asCategorical);
const quasiColumns = quasiIdentifiers
  .filter((q) => q.startsWith(`${declaredType.type}.`))
  .map((q) => q.slice(declaredType.type.length + 1));
const wouldRank = brushCandidates.find((f) => quasiColumns.includes(f.name)) ?? brushCandidates[0];
const asQuantitative = (brushCandidates.find((f) => f.name !== wouldRank?.name) ?? wouldRank)?.name ?? null;

const empty = encodingFor({ types: corpus.types, channels: [], quasiIdentifiers });
ok(
  'an empty list is not an absent key: the writer says the type carries no channel, so there is no colour',
  empty.fill === null,
  `fill=${String(empty.fill)} against ${encoding.fill} for the same corpus with the key absent`,
);

if (asCategorical === null) {
  ok('the corpus has a second drawable column to declare', false, 'nothing but the address, the position and the identity');
} else {
  const declaredColour = encodingFor({
    types: corpus.types,
    channels: [{ name: 'synthetic', column: asCategorical, scale: 'categorical', domain: 3 }],
    quasiIdentifiers,
  });
  ok(
    'a declared categorical IS the colour, over anything the bytes suggest',
    declaredColour.fill === asCategorical,
    `${declaredColour.fill} / declared ${asCategorical} · derived would be ${before}`,
  );
  ok(
    'and the crossfilter projects it, so the view the tab opens has the column it colours by',
    declaredColour.columns.includes(asCategorical),
    `[${declaredColour.columns.join(', ')}]`,
  );

  if (asQuantitative !== null) {
    const declaredChart = encodingFor({
      types: corpus.types,
      channels: [
        { name: 'synthetic', column: asCategorical, scale: 'categorical', domain: 3 },
        { name: 'measure', column: asQuantitative, scale: 'quantitative' },
      ],
      quasiIdentifiers,
    });
    ok(
      'a declared quantitative channel outranks `quasi_identifiers` for the chart',
      declaredChart.brush === asQuantitative,
      `${declaredChart.brush} / declared ${asQuantitative} · the privacy field would have ranked ` +
        `${wouldRank?.name ?? 'nothing'}`,
    );
    ok(
      'and the crossfilter projects it even though no quasi-identifier names it',
      declaredChart.columns.includes(asQuantitative),
      `[${declaredChart.columns.join(', ')}]`,
    );
  }
}

console.log(`\n${failures === 0 ? 'all checks passed' : `${failures} FAILED`}`);
process.exit(failures === 0 ? 0 : 1);
