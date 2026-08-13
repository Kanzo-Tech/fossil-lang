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
 *   2. Every `arguedIn` resolves to another page of this site. A target without an argument behind
 *      it is one person's preference written in the voice of a plan — and an argument kept anywhere
 *      but here is a second reference, which is the thing this site exists to be instead of.
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
 *   - That the page an `arguedIn` names actually *argues* the direction. Same gap as `backedBy`, and
 *     it is the reason the argument pages carry their own admission rule in prose: an entry that
 *     cannot state what would reverse it does not go on `design/discarded`.
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
 * `architecture.mdx` sits at the root next to the index, which carries no registers by design, so
 * the directory rule alone would let it declare a `direction:` block and
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
    const direction = data.direction as { summary?: string; arguedIn?: string } | undefined;
    const today = data.today as
      | { summary?: string; backedBy?: string; unmeasured?: unknown }
      | undefined;

    expect(direction?.summary, "direction.summary: one sentence, where this is going").toBeTruthy();
    expect(direction?.arguedIn, "direction.arguedIn: a /docs/… route on this site").toBeTruthy();
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

/**
 * The frontmatter registers govern one citation per page. The prose carries dozens.
 *
 * A page that says a thing is true at `crates/fossil-hir/src/lower.rs:60` is making the same kind of
 * promise as a `backedBy:`, and it rots the same way — except there are a hundred of them and nobody
 * re-reads a paragraph to check a line number. Every `` `path/to/file.ext:12` `` span in an MDX file
 * has to name a line that exists.
 *
 * This caught three real errors the day it was written, two of them in pages written the same hour:
 * a range whose end ran past the file, and a mistyped path.
 *
 * What it does NOT prove — and the gap is the interesting one: **it checks that the line exists, not
 * that it says what the page claims.** A citation that drifts one line still passes. That failure
 * mode is not hypothetical either; three enum variants were cited at real lines in the right file,
 * permuted. Catching that needs the citation to carry what it asserts, which is a heavier convention
 * than this one and has not earned itself yet.
 */
const CITATION = /`([\w./-]+\.(?:rs|toml|bnf|mjs|ts|tsx|yml|json)):(\d+)(?:-(\d+))?`/g;

interface Citation {
  /** `<page>:<line in the page>` — so a failure message points at the prose, not the target. */
  where: string;
  span: string;
  path: string;
  last: number;
}

const citations: Citation[] = mdxUnder(CONTENT_ROOT).flatMap((file) => {
  const page = relative(repoRoot, file);
  return readFileSync(file, "utf8").split("\n").flatMap((line, index) =>
    [...line.matchAll(CITATION)].map((m) => ({
      where: `${page}:${index + 1}`,
      span: `${m[1]}:${m[2]}${m[3] ? `-${m[3]}` : ""}`,
      path: m[1],
      last: Number(m[3] ?? m[2]),
    })),
  );
});

describe("every inline file:line citation resolves", () => {
  // Same reason as above: a regex that stops matching would turn this into a vacuous pass.
  it("finds citations at all", () => {
    expect(citations.length).toBeGreaterThan(0);
  });

  it.each(citations)("$where cites $span", ({ path, last }) => {
    const target = join(repoRoot, path);
    expect(existsSync(target), `${path} is not on disk`).toBe(true);
    const lines = readFileSync(target, "utf8").split("\n").length;
    expect(last, `${path} has ${lines} lines`).toBeLessThanOrEqual(lines);
  });
});

/**
 * `arguedIn` is a route on this site, and that is the whole of the change from what it replaced.
 *
 * It used to name a file under `decisions/` — a directory of sixty-three records that was the second
 * reference this site was meant to be instead of, and that produced the failure it was written to
 * prevent: a hundred and fifty-nine dead citations, and eighteen of twenty `ADR-0050` references
 * resolving to the wrong record. A path into that directory was checkable only in the weakest sense
 * — the file was there — and it moved the reason for a direction somewhere a reader of this site
 * could not follow.
 *
 * A route is checkable more strictly, and the strictness is the point. The target has to be a page
 * of this site, and it has to be a *different* page: a direction whose argument is the page it is
 * written on has no argument, it has a restatement. Both failures are silent in prose and neither is
 * visible in a diff.
 *
 * Routes are resolved the way fumadocs resolves them, which is not `join`: `/docs/design/corpus` is
 * `content/docs/design/corpus.mdx`, a group folder in brackets is invisible in the URL, and a
 * section index may be either `x/index.mdx` or `x.mdx`. All three shapes are in this tree today.
 */
function pageFileForRoute(route: string): string | null {
  const rest = route.replace(/^\/docs\/?/, "");
  const candidates = [
    join(CONTENT_ROOT, `${rest}.mdx`),
    join(CONTENT_ROOT, rest, "index.mdx"),
    // `(root)/index.mdx` is `/docs`, and `(root)/architecture.mdx` is `/docs/architecture`:
    // fumadocs strips a bracketed folder from the URL, so a route may live one level in.
    join(CONTENT_ROOT, "(root)", `${rest || "index"}.mdx`),
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? null;
}

describe("every cited path is still there", () => {
  it.each(pages)("$id names an argument on this site", ({ path, data }) => {
    const arguedIn = (data.direction as { arguedIn?: string } | undefined)?.arguedIn as string;

    expect(arguedIn.startsWith("/docs/"), `${arguedIn} must be a route on this site`).toBe(true);

    const target = pageFileForRoute(arguedIn);
    expect(target, `${arguedIn} does not resolve to a page under content/docs/`).not.toBeNull();
    expect(target, `${arguedIn} is the page itself, which argues nothing`).not.toBe(path);
  });

  it.each(pages.filter((p) => (p.data.today as { backedBy?: string })?.backedBy))(
    "$id cites evidence that exists",
    ({ data }) => {
      const backedBy = (data.today as { backedBy?: string }).backedBy as string;
      expect(existsSync(join(repoRoot, backedBy)), `${backedBy} is not on disk`).toBe(true);
    },
  );
});

/**
 * The grammar-citation guard: a citation names a PRODUCTION, never a line.
 *
 * `grammar.bnf` is one of the two normative documents about the language, and the tree cited it a
 * hundred and thirty times as `grammar.bnf:NNN`. Every one of those was already stale before anyone
 * touched anything — they were written against a layout that predates the 656-line file — and the
 * rewrite moved them all again. The rot was self-inflicted, and this is the diagnosis: **nothing
 * cited a production BY ITS NAME.** A line number is not a name. It is an offset into a file that
 * is edited by definition, it is invalidated by an insertion twenty lines above it, and the failure
 * is silent: the citation still resolves, to different text.
 *
 * So the spelling is `grammar.bnf, <anchor>`, and the anchor is one of exactly two things:
 *
 *   1. **A name the file defines** — anything on the left of a `:=`, terminal or production:
 *      `grammar.bnf, TypeDef`, `grammar.bnf, ShapeExpr`, `grammar.bnf, AT_ATTR`.
 *   2. **A banner section** — `grammar.bnf, § RESERVED KEYWORDS`, cited up to the parenthetical:
 *      the file writes `(* ═══ RESERVED KEYWORDS (cannot be identifiers) ═══ *)` and the heading is
 *      the part before the `(`. Seven of these exist and they are where the file states a rule that
 *      no single production carries.
 *
 * One anchor per citation. Two anchors are two citations; a comma-separated list would let ordinary
 * prose after the comma pass for a name, which is the parse that lets a guard fail open.
 *
 * The comma binds to the filename, so a citation is `grammar.bnf, TypeDef` or `` `grammar.bnf,
 * TypeDef` `` and never `` `grammar.bnf`, TypeDef ``. That is what keeps a citation distinct from
 * the dozens of sentences that merely mention the file — `` «a tombstone in `grammar.bnf`» `` is
 * prose, and this guard has no business reading it as a claim about a production.
 *
 * Three assertions:
 *
 *   1. No `grammar.bnf:NNN` survives anywhere in scope. The old spelling is not deprecated, it is
 *      banned — this repo keeps no compatibility, and an accepted second spelling is how the first
 *      one comes back.
 *   2. Every `grammar.bnf, X` names something `grammar.bnf` defines today.
 *   3. Both kinds are found at all, and the file yields anchors at all. A regex that quietly stops
 *      matching turns a guard into a vacuous pass, which is worse than not having one.
 *
 * **Why this is not the design of the `file:line` guard above it, whose own docblock confesses that
 * it checks that a line exists and not that the line says what is claimed.** That gap was not
 * hypothetical: three enum variants were cited at real lines of the right file, permuted, and
 * passed. The permutation failure cannot happen here, and that is the whole of the improvement — an
 * anchor is a name, so reordering `grammar.bnf`, inserting a production, or rewriting every comment
 * in it cannot make a citation point somewhere else. It either names something the file defines or
 * it does not, and the check is total over the tree rather than over one directory of prose.
 *
 * **What it still CANNOT prove, and the residue is real:**
 *
 *   - **That the production says what the citation claims.** `grammar.bnf, MulExpr` next to a
 *     sentence about the ternary passes here. The anchor is checkable; the assertion attached to it
 *     is not, and no test of this shape will ever make it so. What changed is that a WRONG anchor is
 *     now a wrong NAME — visible to a reader who knows the grammar — instead of a number nobody can
 *     evaluate by eye.
 *   - **That a citation should have been there at all.** The conversion deleted every citation that
 *     pointed at a tombstone — `PIPE`, `TEMPLATE`, `ABS_IRI`, `FieldRef`, `PrefixDecl` and the rest
 *     of the forms the file declares absent — because the file defines no name for a thing that does
 *     not exist, and the surrounding comment already stated the rule. This guard cannot tell a
 *     comment that lost its citation and kept its meaning from one that lost both.
 *   - **That `grammar.bnf` is right.** It specifies the language; the parser implements it. A
 *     production here and absent from `crates/fossil-syntax` is work outstanding, and that inversion
 *     is deliberate. Nothing mechanical compares the two — the 23 programs of `apps/docs/programs/`
 *     are the only check the file has.
 *
 * **What it does not scan, and why:** `decisions/`, which is being deleted outright, and
 * `SURFACE-PLAN.md`, which has an owner. Both still carry the old spelling. Widening `CITED_TREES`
 * is the whole of the change when that stops being true.
 */
const GRAMMAR = join(repoRoot, "grammar.bnf");

/** Where a `grammar.bnf` citation may appear. Anything outside this is unchecked, not permitted. */
const CITED_TREES: ReadonlyArray<readonly [dir: string, ext: string]> = [
  ["crates", ".rs"],
  ["apps/docs/content", ".mdx"],
];

/** `grammar.bnf, TypeDef` or `grammar.bnf, § RESERVED KEYWORDS`. Nothing else is a citation. */
const GRAMMAR_CITATION =
  /grammar\.bnf,\s*(?:§\s*(?<section>[A-Z]+(?: [A-Z]+)*)|(?<name>[A-Za-z_][A-Za-z0-9_]*))/g;

/**
 * The two spellings this guard exists to keep out, and neither is checkable.
 *
 * A line number is the one the conversion removed. The bare `§` — `grammar.bnf §"DISAMBIGUATION
 * RULES"`, with no comma — is the older section citation, and it is banned for the reason a second
 * spelling is always banned here: the checked form and the unchecked form would look alike, so the
 * unchecked one would spread. Only `grammar.bnf, § …` is read.
 *
 * The optional backtick is not decoration: `` `grammar.bnf`:462-469 `` was in the tree, and a
 * pattern anchored on the bare filename would have walked straight past it.
 */
const GRAMMAR_STALE_CITATION = /grammar\.bnf`?\s*(?::\s*\d+|§)/;

function filesUnder(dir: string, ext: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((entry) => {
    if (entry === "target" || entry === "node_modules") return [];
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) return filesUnder(full, ext);
    return entry.endsWith(ext) ? [full] : [];
  });
}

const citingFiles = CITED_TREES.flatMap(([dir, ext]) => filesUnder(join(repoRoot, dir), ext));

const grammarSource = readFileSync(GRAMMAR, "utf8").split("\n");

/**
 * Every name the file DEFINES — the left of a `:=`, terminals included, and a comma list on the
 * left defines both (`LPAREN, RPAREN := '(' ')'`).
 *
 * A name only mentioned in prose is not defined, and that is the point rather than an oversight: the
 * tombstones name `PIPE`, `FieldRef` and `PrefixedName` in order to say they are gone, and a
 * citation of one has to fail.
 */
const defined = new Set(
  grammarSource.flatMap((line) => {
    const lhs = /^([A-Za-z_][A-Za-z0-9_]*(?:,\s*[A-Za-z_][A-Za-z0-9_]*)*)\s*:=/.exec(line);
    return lhs ? lhs[1].split(",").map((name) => name.trim()) : [];
  }),
);

/** The banner sections, cut at the parenthetical or the dash so the citable heading is stable. */
const sections = new Set(
  grammarSource.flatMap((line) => {
    const banner = /^\(\*\s*═+\s+(.+?)\s+═+\s*\*\)\s*$/.exec(line);
    return banner ? [banner[1].split(/\s*[(—]/)[0].trim()] : [];
  }),
);

interface GrammarCitation {
  /** `<file>:<line>` — a failure names the comment, not the grammar. */
  where: string;
  anchor: string;
  kind: "production" | "section";
}

const grammarCitations: GrammarCitation[] = citingFiles.flatMap((file) => {
  const id = relative(repoRoot, file);
  return readFileSync(file, "utf8").split("\n").flatMap((line, index) =>
    [...line.matchAll(GRAMMAR_CITATION)].map((m) => ({
      where: `${id}:${index + 1}`,
      anchor: (m.groups?.section ?? m.groups?.name) as string,
      kind: m.groups?.section ? ("section" as const) : ("production" as const),
    })),
  );
});

describe("every grammar.bnf citation names a production", () => {
  // Without these three, a regex that stopped matching would report a clean sweep of nothing.
  it("grammar.bnf defines names to cite", () => {
    expect(defined.size).toBeGreaterThan(50);
    expect(defined.has("TypeDef")).toBe(true);
    expect(sections.has("RESERVED KEYWORDS")).toBe(true);
  });

  it("finds citations of both kinds", () => {
    expect(grammarCitations.filter((c) => c.kind === "production").length).toBeGreaterThan(0);
    expect(grammarCitations.filter((c) => c.kind === "section").length).toBeGreaterThan(0);
  });

  it("finds files to check", () => {
    expect(citingFiles.length).toBeGreaterThan(100);
  });

  it.each(citingFiles.map((file) => ({ id: relative(repoRoot, file), file })))(
    "$id spells its citations the one way",
    ({ file }) => {
      const offender = readFileSync(file, "utf8")
        .split("\n")
        .findIndex((line) => GRAMMAR_STALE_CITATION.test(line));
      expect(
        offender,
        `line ${offender + 1} cites grammar.bnf by a spelling this guard cannot check; ` +
          "write `grammar.bnf, <production>` or `grammar.bnf, § <SECTION>`",
      ).toBe(-1);
    },
  );

  it.each(grammarCitations)("$where cites $anchor", ({ anchor, kind }) => {
    if (kind === "section") {
      expect(sections.has(anchor), `grammar.bnf has no § ${anchor}`).toBe(true);
    } else {
      expect(defined.has(anchor), `grammar.bnf defines no ${anchor}`).toBe(true);
    }
  });
});
