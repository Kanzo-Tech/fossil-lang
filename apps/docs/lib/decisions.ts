import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot } from "@/lib/repo";

export interface DecisionRecord {
  /** File name under `decisions/`, which is the identifier the ADRs cite each other by. */
  file: string;
  /** `0044`, or `null` for the non-numbered logs the directory also holds. */
  number: string | null;
  title: string;
  status: string | null;
  date: string | null;
}

const DECISIONS_DIR = "decisions";

// `# ADR 0044: Tres anillos, …` — and `# rudof on wasm32… — Phase 0 spike outcome` for the two
// non-numbered logs, which is why the ADR prefix is optional rather than a second parser.
const HEADING = /^#\s+(?:ADR\s+(\d{4})\s*:\s*)?(.+)$/m;
const FIELD = (name: string) => new RegExp(`^\\*\\*${name}:\\*\\*\\s*(.+)$`, "m");

/**
 * Read `decisions/` from disk at build time.
 *
 * The index is derived, never transcribed. `decisions/README.md` keeps a hand-maintained table and
 * it is already behind — its last row is 0039, and 0040 through 0045 are on disk — which is the
 * whole argument for not writing a second one by hand on this site.
 */
export function readDecisions(): DecisionRecord[] {
  const dir = join(repoRoot, DECISIONS_DIR);

  return readdirSync(dir)
    .filter((file) => file.endsWith(".md"))
    // README is the directory's own front matter and template is a blank to copy; neither is a
    // decision, and listing them would make the count wrong.
    .filter((file) => file !== "README.md" && file !== "template.md")
    .map((file) => {
      const source = readFileSync(join(dir, file), "utf8");
      const heading = HEADING.exec(source);
      return {
        file: `${DECISIONS_DIR}/${file}`,
        number: heading?.[1] ?? null,
        title: heading?.[2]?.trim() ?? file,
        status: FIELD("Status").exec(source)?.[1]?.trim() ?? null,
        date: FIELD("Date").exec(source)?.[1]?.trim() ?? null,
      };
    })
    .sort((a, b) => (a.number ?? "9999").localeCompare(b.number ?? "9999"));
}
