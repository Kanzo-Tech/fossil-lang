/**
 * `fossil.json`, read with `JSON.parse` and nothing else.
 *
 * The manifest is the one file a reader opens before any Parquet, and it is JSON so that reading it
 * is a standard library call in every language a stranger might check a corpus from. This module
 * does not validate it: it reads it and says where it could not. What a field must hold is each
 * guard's question, asked where the guard can name what broke.
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

/** The file every reader starts at, relative to the corpus root. */
export const ENTRY_POINT = "fossil.json";

/** The one format this checker reads. A corpus declaring another is refused before any byte of it. */
export const FORMAT = "fossil/1";

/**
 * The manifest at `root`, parsed — or why it could not be.
 *
 * @param {string} root
 * @returns {{ path: string, json: any | null, error: string | null }}
 */
export function load(root) {
  const path = join(root, ENTRY_POINT);
  if (!existsSync(path)) return { path, json: null, error: `${ENTRY_POINT} is not on disk` };
  try {
    return { path, json: JSON.parse(readFileSync(path, "utf8")), error: null };
  } catch (cause) {
    return { path, json: null, error: `${ENTRY_POINT} is not JSON: ${cause.message}` };
  }
}
