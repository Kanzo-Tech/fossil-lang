import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";

import { initFossilStorage } from "@fossil-lang/storage";

await initFossilStorage(
  await readFile(
    createRequire(import.meta.url).resolve("@fossil-lang/storage/pkg/fossil_storage_wasm_bg.wasm"),
  ),
);
