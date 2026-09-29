// Puts the `.mdx` source files at the URLs the server build serves them from, after the export.
//
//   node scripts/materialise-mdx.mjs [--out <dir>]
//
// `/docs/book/anatomy.mdx` returns that page's Markdown source. On a server a rewrite sends it to
// `/llms.mdx/docs/book/anatomy.mdx`; a static export has nowhere to run a rewrite, so the file has
// to BE at the address. That is all this does — it moves the exported tree up one level and deletes
// the staging directory. The handlers cannot live at the address themselves: `app/docs/[[...slug]]`
// already owns `/docs/*`, and a route handler in the same segment is a conflict Next refuses.
//
// It fails loudly rather than warning. A missing source file is invisible on the site — the pages
// all render — and the audience for these URLs is agents reading `llms.txt`, who will not report it.

import { cp, mkdir, readdir, rename, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const at = args.indexOf("--out");
const OUT = resolve(at >= 0 ? args[at + 1] : join(HERE, "..", "out"));

const STAGING = join(OUT, "llms.mdx");
const INDEX_SOURCE = join(STAGING, "docs-index");
const INDEX_TARGET = join(OUT, "docs.mdx");

const die = (message) => {
  console.error(`materialise-mdx: ${message}`);
  process.exit(1);
};

if (!existsSync(OUT)) die(`no export at ${OUT} — run \`next build\` with DOCS_STATIC_EXPORT=1 first`);
if (!existsSync(STAGING)) die(`${STAGING} is missing, so the .mdx routes did not export`);
if (!existsSync(INDEX_SOURCE)) die(`${INDEX_SOURCE} is missing — app/llms.mdx/docs-index did not export`);

/** Every file under `dir`, absolute, depth-first. */
async function walk(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await walk(path)));
    else out.push(path);
  }
  return out;
}

const staged = await walk(join(STAGING, "docs"));
const sources = staged.filter((f) => f.endsWith(".mdx"));

// An EQUALITY against the content tree rather than a floor: the assertions below are about files
// being present, and an empty staging directory would satisfy them by vacuum. Minus one, because
// the index page's source is emitted by its own route and moved separately below.
const pages = (await walk(join(HERE, "..", "content", "docs"))).filter((f) => f.endsWith(".mdx"));
if (sources.length !== pages.length - 1) {
  die(
    `${sources.length} .mdx sources staged for ${pages.length} content pages (expected ${pages.length - 1} plus the index).\n` +
      `A partial export would publish a site whose pages all render and whose sources 404.`,
  );
}
if (sources.length !== staged.length) {
  const strays = staged.filter((f) => !f.endsWith(".mdx")).slice(0, 5);
  die(`${staged.length - sources.length} staged file(s) do not end in .mdx, e.g. ${strays.join(", ")}`);
}

// Merge rather than rename the directory: `out/docs/` already holds the HTML pages. A page and its
// source differ by an extension, so nothing collides — `out/docs/design/` is the section's
// directory and `out/docs/design.mdx` its source.
for (const file of sources) {
  const target = join(OUT, "docs", file.slice(join(STAGING, "docs").length + 1));
  if (existsSync(target)) die(`${target} already exists — the export and this script disagree about the layout`);
  await mkdir(dirname(target), { recursive: true });
  await cp(file, target);
}

await rename(INDEX_SOURCE, INDEX_TARGET);
await rm(STAGING, { recursive: true, force: true });

// The Actions deploy serves the artefact as it is and never runs Jekyll, but a branch-based Pages
// setup does, and Jekyll drops every directory beginning with an underscore — `_next/`, which is
// all the JavaScript and all the CSS. One empty file is cheaper than that failure.
await writeFile(join(OUT, ".nojekyll"), "");

console.log(`materialise-mdx: ${sources.length} sources placed under docs/, plus docs.mdx and .nojekyll`);
