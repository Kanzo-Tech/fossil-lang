#!/usr/bin/env node
// Post-build step: tsc emits compiled JS + .d.ts to dist/ but does NOT copy
// the raw .fossil / .csv / .csvw.json / .shex fixture files referenced via
// `import './hello.fossil?raw'`. Without this script, consumers loading
// `dist/hello/index.js` (via the published `main` field) would fail to
// resolve the relative `?raw` paths because the raw files live in `src/hello/`,
// not `dist/hello/`.
//
// This script mirrors `src/hello/*.{fossil,csv,csvw.json,shex}` into
// `dist/hello/`. It runs as part of `pnpm build` (chained via npm script).

import { readdirSync, mkdirSync, copyFileSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const pkgRoot = join(__dirname, '..');

const RAW_EXTENSIONS = new Set(['.fossil', '.csv', '.shex']);
// .csvw.json gets matched separately via suffix check (avoids confusing
// fixture JSON with a tsc-generated JSON output file, if there were any).

/** Recursively copy raw fixture files from src/ to dist/, preserving structure. */
function copyTree(srcDir, dstDir) {
  for (const entry of readdirSync(srcDir, { withFileTypes: true })) {
    const srcPath = join(srcDir, entry.name);
    const dstPath = join(dstDir, entry.name);
    if (entry.isDirectory()) {
      mkdirSync(dstPath, { recursive: true });
      copyTree(srcPath, dstPath);
      continue;
    }
    const lower = entry.name.toLowerCase();
    const isRawExt = [...RAW_EXTENSIONS].some(ext => lower.endsWith(ext));
    const isCsvw = lower.endsWith('.csvw.json');
    if (!isRawExt && !isCsvw) continue;
    copyFileSync(srcPath, dstPath);
    console.log(`copied: ${srcPath} -> ${dstPath}`);
  }
}

const srcRoot = join(pkgRoot, 'src');
const dstRoot = join(pkgRoot, 'dist');
if (!statSync(srcRoot).isDirectory()) {
  console.error(`error: ${srcRoot} is not a directory`);
  process.exit(1);
}
mkdirSync(dstRoot, { recursive: true });
copyTree(srcRoot, dstRoot);
