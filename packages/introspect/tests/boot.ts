import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";

import { initStorage } from "@fossil-lang/storage";

await initStorage(
  await readFile(
    createRequire(import.meta.url).resolve("@fossil-lang/storage/pkg/fossil_storage_wasm_bg.wasm"),
  ),
);
