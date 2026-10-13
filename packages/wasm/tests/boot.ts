import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { initFossilStorage } from "@fossil-lang/storage";

import { initFossilWasm } from "../src/index.js";

const resolve = createRequire(import.meta.url).resolve;
await initFossilStorage(await readFile(resolve("@fossil-lang/storage/pkg/fossil_storage_wasm_bg.wasm")));
await initFossilWasm(await readFile(fileURLToPath(new URL("../pkg/fossil_wasm_bg.wasm", import.meta.url))));
