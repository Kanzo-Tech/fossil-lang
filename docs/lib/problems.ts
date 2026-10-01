import { readFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot } from "@/lib/repo";

/** One live code of the catalogue, as `Problem`'s derived JSON Schema states it. */
export interface CatalogueEntry {
  /** `area/kind` — the serde tag, and the route of its page under `/docs/errors/`. */
  code: string;
  /** Fixed per code (RFC 9457 §3.1.3); a page's title is this, character for character. */
  title: string;
}

/**
 * The catalogue, read from `crates/fossil-graph-schema/problem.schema.json`.
 *
 * That file is the derived one: `tests/problem_schema.rs` holds it equal to `Problem`, and
 * `problem.gen.ts` is generated from it, so this is the same source the TypeScript union comes
 * from rather than a third reading of `problem.rs`. The error index renders it, and the
 * codes-to-pages suite in `content.test.ts` holds the pages against it.
 */
export function catalogue(): CatalogueEntry[] {
  const schema = JSON.parse(
    readFileSync(join(repoRoot, "crates/fossil-graph-schema/problem.schema.json"), "utf8"),
  ) as { oneOf: { title: string; properties: { code: { enum: string[] } } }[] };
  return schema.oneOf.map((arm) => ({
    code: arm.properties.code.enum[0] as string,
    title: arm.title,
  }));
}
