import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import matter from "gray-matter";
import { describe, expect, it } from "vitest";
import { repoRoot } from "@/lib/repo";

/**
 * The editorial guard.
 *
 * This site says two things about every characteristic and every protocol, and keeps them apart:
 * where fossil is going, and what is true today. Prose cannot hold that line on its own — a target
 * quietly restated as a fact is the exact failure this whole site exists to avoid, and it is
 * invisible in a diff. So the line is a test.
 *
 * Three assertions, and each one buys something a reader could otherwise be wrong about:
 *
 *   1. Both registers are present. A page with only `direction:` reads as vapour; a page with only
 *      `today:` reads as a changelog. Neither is this site.
 *   2. Every `decidedBy` names a file under `decisions/`. A target without a decision behind it is
 *      one person's preference written in the voice of a plan.
 *   3. Every `backedBy` exists on disk. This is the one that catches the real drift: a test gets
 *      renamed, the claim it backed keeps its confident sentence, and nothing anywhere notices.
 *      Renaming that test now fails the docs build, because `build` runs this first.
 *
 * A fourth assertion joined them and is of a different kind: it does not check a citation, it *is*
 * the evidence one page cites — see `fossilDependenciesOf` below and `architecture.mdx`, whose
 * `backedBy` points here. A page may cite this file only for a claim this file actually measures.
 *
 * What it does NOT prove, and this matters:
 *
 *   - That a `backedBy` path actually *tests* the claim. It checks that the file is there, not that
 *     it asserts anything. A path to a source file that merely contains the feature passes here,
 *     and two of them do today — see `bounded-write.mdx`, whose spill test ADR-0043 stage 4 owes
 *     and has not written. The page says so in its own prose; a reader gets the truth, the guard
 *     only gets the path.
 *   - That either sentence is *true*. No test can. What it can do is make the citation falsifiable,
 *     which is the difference between a claim and an assertion.
 *   - Anything about `protocols/`, which has no pages yet. The directory is in the list so the first
 *     one arrives already governed, rather than governed later by someone remembering.
 */

const CONTENT_ROOT = join(process.cwd(), "content/docs");

/** The two directories the stance governs. `protocols/` is empty in this phase and still listed. */
const GOVERNED = ["characteristics", "protocols"];

interface Page {
  /** Repo-relative, so a failure message names the file the way `git` and the ADRs do. */
  id: string;
  path: string;
  data: Record<string, unknown>;
}

function mdxUnder(dir: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) return mdxUnder(full);
    return entry.endsWith(".mdx") ? [full] : [];
  });
}

function read(path: string): Page {
  return {
    id: relative(repoRoot, path),
    path,
    data: matter(readFileSync(path, "utf8")).data as Record<string, unknown>,
  };
}

const inGovernedDirs = GOVERNED.flatMap((group) => mdxUnder(join(CONTENT_ROOT, group))).map(read);

/**
 * And anywhere else, any page that opts in.
 *
 * `architecture.mdx` sits at the root next to the index and the decision list, both of which carry
 * no registers by design, so the directory rule alone would let it declare a `direction:` block and
 * then never be held to it — the one page on this site whose whole subject is a shape that does not
 * exist yet. Declaring either block is the opt-in; declaring one and not the other is the failure.
 */
const opted = mdxUnder(CONTENT_ROOT)
  .filter((path) => !inGovernedDirs.some((page) => page.path === path))
  .map(read)
  .filter((page) => "direction" in page.data || "today" in page.data);

const pages: Page[] = [...inGovernedDirs, ...opted];

describe("every governed page declares both registers", () => {
  // A directory rename that emptied `characteristics/` would otherwise turn every assertion below
  // into a vacuous pass over zero pages, which is the classic way a guard stops guarding.
  it("finds pages to govern at all", () => {
    expect(pages.length).toBeGreaterThan(0);
  });

  it.each(pages)("$id declares direction and today", ({ data }) => {
    const direction = data.direction as { summary?: string; decidedBy?: string } | undefined;
    const today = data.today as
      | { summary?: string; backedBy?: string; unmeasured?: unknown }
      | undefined;

    expect(direction?.summary, "direction.summary: one sentence, where this is going").toBeTruthy();
    expect(direction?.decidedBy, "direction.decidedBy: a file under decisions/").toBeTruthy();
    expect(today?.summary, "today.summary: one sentence, what is true right now").toBeTruthy();

    // Exactly one, never both, never neither. `unmeasured: true` is the honest way to say there is
    // no evidence; a page may not claim evidence and disclaim it in the same breath.
    const hasBacking = typeof today?.backedBy === "string" && today.backedBy.length > 0;
    const declaresUnmeasured = today?.unmeasured === true;
    expect(
      [hasBacking, declaresUnmeasured].filter(Boolean),
      "today: exactly one of `backedBy: <path>` or `unmeasured: true`",
    ).toHaveLength(1);
  });
});

/**
 * The one claim on this site that a manifest can settle, and `architecture.mdx` is what cites it.
 *
 * That page says `fossil-graph` reaches the rest of the tree exactly once, and everything else it
 * says about two cores hangs off that number. Left as prose it is a sentence somebody measured in
 * August 2026; here it is a build failure the moment it stops holding — in either direction, which
 * is the point. A second dependency appearing means the graph core has started to grow roots into
 * the language; the last one *disappearing* means ADR-0042 §2 landed and the page's `today:` block
 * now understates what is true. Both deserve a red test, because both need the page rewritten.
 *
 * `[dev-dependencies]` are deliberately out of scope: a test may depend on whatever it likes, and
 * `fossil-graph`'s do not include a fossil crate today anyway.
 */
const GRAPH_MANIFEST = "crates/fossil-graph/Cargo.toml";

/** Enough of a TOML reader for one question: the `fossil-*` keys under `[dependencies]`. */
function fossilDependenciesOf(manifest: string): string[] {
  let section = "";
  const found: string[] = [];

  for (const raw of readFileSync(manifest, "utf8").split("\n")) {
    const line = raw.trim();
    if (line.startsWith("#")) continue;

    const header = /^\[([^\]]+)\]/.exec(line);
    if (header) {
      section = header[1];
      continue;
    }

    const key = /^(fossil-[a-z0-9-]+)\s*=/.exec(line);
    if (section === "dependencies" && key) found.push(key[1]);
  }

  return found.sort();
}

describe("the graph core reaches the rest of the tree exactly once", () => {
  it("is a manifest that is still on disk", () => {
    expect(existsSync(join(repoRoot, GRAPH_MANIFEST)), `${GRAPH_MANIFEST} is not on disk`).toBe(
      true,
    );
  });

  it("fossil-graph depends on fossil-sinks and on nothing else of ours", () => {
    expect(fossilDependenciesOf(join(repoRoot, GRAPH_MANIFEST))).toEqual(["fossil-sinks"]);
  });
});

describe("every cited path is still there", () => {
  it.each(pages)("$id cites a decision that exists", ({ data }) => {
    const decidedBy = (data.direction as { decidedBy?: string } | undefined)?.decidedBy as string;
    expect(decidedBy.startsWith("decisions/"), `${decidedBy} must live under decisions/`).toBe(true);
    expect(existsSync(join(repoRoot, decidedBy)), `${decidedBy} is not on disk`).toBe(true);
  });

  it.each(pages.filter((p) => (p.data.today as { backedBy?: string })?.backedBy))(
    "$id cites evidence that exists",
    ({ data }) => {
      const backedBy = (data.today as { backedBy?: string }).backedBy as string;
      expect(existsSync(join(repoRoot, backedBy)), `${backedBy} is not on disk`).toBe(true);
    },
  );
});
