import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";

import { initFossilStorage } from "@fossil-lang/storage";
import { initFossilWasm } from "@fossil-lang/wasm";

const resolve = createRequire(import.meta.url).resolve;
await initFossilStorage(await readFile(resolve("@fossil-lang/storage/pkg/fossil_storage_wasm_bg.wasm")));
await initFossilWasm(await readFile(resolve("@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm")));
