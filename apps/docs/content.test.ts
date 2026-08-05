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

const pages: Page[] = GOVERNED.flatMap((group) =>
  mdxUnder(join(CONTENT_ROOT, group)).map((path) => ({
    id: relative(repoRoot, path),
    path,
    data: matter(readFileSync(path, "utf8")).data as Record<string, unknown>,
  })),
);

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
