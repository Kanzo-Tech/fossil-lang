#!/usr/bin/env node
/**
 * The corpus checker.
 *
 *   node guards/check.mjs <corpus-dir> [--only id,id] [--explain]
 *   node guards/check.mjs --explain          # what each guard proves, without a corpus
 *
 * Exit 0 when every convention holds, 1 when one does not, 2 when the checker could not run.
 *
 * It needs `node` and the `duckdb` binary. It does not need fossil, Rust, pnpm, or this repository
 * — copy the `guards/` directory and it runs against any corpus, which is the point of documenting
 * a format instead of shipping a type for it.
 */

import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { available } from "./duck.mjs";
import { inspect } from "./inspect.mjs";
import { GUARDS, runAll } from "./guards.mjs";

const argv = process.argv.slice(2);
const flag = (name) => argv.includes(`--${name}`);
const value = (name) => {
  const at = argv.indexOf(`--${name}`);
  return at === -1 ? null : argv[at + 1];
};
const positional = argv.filter((a, i) => !a.startsWith("--") && !argv[i - 1]?.startsWith("--"));

function explain() {
  for (const guard of GUARDS) {
    console.log(`\n\x1b[1m${guard.id}\x1b[0m — ${guard.title}`);
    console.log(`  proves        ${guard.proves.replace(/\s+/g, " ")}`);
    console.log(`  cannot prove  ${guard.cannotProve.replace(/\s+/g, " ")}`);
  }
}

if (flag("explain") && positional.length === 0) {
  explain();
  process.exit(0);
}

const root = positional[0] ? resolve(positional[0]) : null;
if (!root) {
  console.error("usage: node guards/check.mjs <corpus-dir> [--only id,id] [--explain]");
  process.exit(2);
}
if (!existsSync(root)) {
  console.error(`no such directory: ${root}`);
  process.exit(2);
}

const duck = available();
if (!duck) {
  console.error(
    "the `duckdb` binary is not on PATH. The checker needs it and nothing else — see " +
      "https://duckdb.org/docs/installation.",
  );
  process.exit(2);
}

let corpus;
try {
  corpus = inspect(root);
} catch (error) {
  console.error(`the corpus could not be read: ${error.message}`);
  process.exit(2);
}

const only = value("only")?.split(",").map((s) => s.trim());
const results = runAll(corpus, only ?? null);

console.log(`corpus  ${root}`);
console.log(`duckdb  ${duck.split("\n")[0]}`);
console.log(
  `shape   ${corpus.types.length} vertex type(s), ${corpus.edges.length} edge type(s), ` +
    `${corpus.types.map((t) => t.layout).join("/") || "—"}\n`,
);

let failed = 0;
for (const { guard, failures, notes, ms } of results) {
  const mark = failures.length === 0 ? "\x1b[32m✓\x1b[0m" : "\x1b[31m✗\x1b[0m";
  console.log(`${mark} ${guard.id.padEnd(24)} ${guard.title}  \x1b[2m${ms} ms\x1b[0m`);
  for (const note of notes) console.log(`    \x1b[2m· ${note}\x1b[0m`);
  // A broken convention usually breaks in every tile at once, and a thousand identical lines hide
  // the next guard's one line. The count is the finding; the examples are the lead.
  for (const failure of failures.slice(0, 6)) console.log(`    \x1b[31m${failure}\x1b[0m`);
  if (failures.length > 6) console.log(`    \x1b[31m…and ${failures.length - 6} more\x1b[0m`);
  if (failures.length > 0) {
    failed += 1;
    console.log(`    \x1b[2mcannot prove: ${guard.cannotProve.replace(/\s+/g, " ")}\x1b[0m`);
  }
}

console.log(
  `\n${results.length - failed}/${results.length} conventions hold` +
    (failed === 0 ? "" : ` · ${failed} broken`),
);
if (flag("explain")) explain();
process.exit(failed === 0 ? 0 : 1);
