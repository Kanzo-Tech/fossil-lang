import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import matter from "gray-matter";
import { describe, expect, it } from "vitest";
import { repoRoot } from "@/lib/repo";

/**
 * The editorial guard.
 *
 * A page that describes something not yet built declares where it is going, and names the page here
 * that argues for it. Everything else describes what is there. Prose cannot hold that line on its
 * own — a destination quietly restated as a fact is the exact failure this site exists to avoid,
 * and it is invisible in a diff. So the line is a test.
 *
 * Two assertions, and each buys something a reader could otherwise be wrong about:
 *
 *   1. A declared `direction:` is complete: a summary and an argument. Half a block is a page that
 *      gestures at a future and never says who would settle it.
 *   2. Every `arguedIn` resolves to another page of this site. A destination with no argument behind
 *      it is one person's preference written in the voice of a plan — and an argument kept anywhere
 *      but here is a second reference, which is the thing this site exists to be instead of.
 *
 * A third assertion joined them and is of a different kind: it does not check a citation, it *is*
 * the evidence one page cites — see `fossilDependenciesOf` below. A page may cite this file only for
 * a claim this file actually measures.
 *
 * WHAT WAS HERE AND IS NOT, because the deletion is the load-bearing part.
 *
 * There was a second register, `today:`, with a `backedBy` path to a file that would go red if the
 * sentence stopped holding. Both are gone. The guard behind `backedBy` could prove the file existed
 * and nothing else, and the measurement is unambiguous: of forty-nine `file:line` citations, eight
 * had drifted to a line that says something different — one to a blank line, one fifty-three lines
 * adrift, one naming an enum that had moved — with CI green the whole time. And `unmeasured: true`,
 * the honest alternative the schema offered, was used by exactly zero of nine pages: every one of
 * them preferred a weak citation to admitting there was no evidence. A field that cannot fail when
 * it is wrong is not evidence, and one nobody uses honestly is not an escape hatch.
 *
 * What replaced it is not a wider guard. It is `<Program src= region= />`, which reads the file at
 * build time: a region that no longer exists stops the build, and the code on the page is the code
 * on disk rather than a transcription of it. That is the property `backedBy` was reaching for.
 *
 * What this still does NOT prove:
 *
 *   - That the sentence is *true*. No test can. What it can do is make the argument locatable.
 *   - That the page an `arguedIn` names actually *argues* the direction. It is the reason the
 *     argument pages carry their own admission rule in prose: an entry that cannot state what would
 *     reverse it does not go on `design/discarded`.
 */

const CONTENT_ROOT = join(process.cwd(), "content/docs");

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

/**
 * Declaring a direction is the opt-in, and there is no governed directory.
 *
 * There used to be one — `characteristics/`, plus `protocols/`, which never had a page and sat in
 * the list anyway. Both are gone: `characteristics/` was the section that existed to carry the two
 * registers, every one of its pages named a `design/` page as its `arguedIn`, and with one register
 * left it folded into the pages it was already pointing at. A directory rule needs a directory.
 *
 * So the rule is now the honest one it was always trying to be: a page owes an argument because it
 * claims a future, not because of where it sits in the tree.
 */
const pages: Page[] = mdxUnder(CONTENT_ROOT)
  .map(read)
  .filter((page) => "direction" in page.data);

describe("a declared direction is complete", () => {
  // Opt-in has a failure mode a directory rule did not: if the last `direction:` is deleted, every
  // assertion below passes over zero pages and the guard reports green having checked nothing. This
  // is the classic way a guard stops guarding, and it is measured in this repo rather than feared —
  // `alpha-steps` matched a corpus of zero for a while over in kanzo-ui.
  it("finds a page that declares one at all", () => {
    expect(pages.length).toBeGreaterThan(0);
  });

  it.each(pages)("$id declares a summary", ({ data }) => {
    const direction = data.direction as { summary?: string; arguedIn?: string } | undefined;

    expect(direction?.summary, "direction.summary: one sentence, where this is going").toBeTruthy();
  });
});

/**
 * The one claim on this site that a manifest can settle, and `architecture.mdx` is what cites it.
 *
 * That page says `fossil-graph` reaches the rest of the tree exactly once, and everything else it
 * says about two cores hangs off that number. Left as prose it is a sentence somebody measured in
 * August 2026; here it is a build failure the moment it stops holding — in either direction, which
 * is the point. A second dependency appearing means the graph core has started to grow roots into
 * the language; the last one *disappearing* means the graph core has been cut loose entirely and
 * the page's `today:` block now understates what is true. Both deserve a red test, because both need the page rewritten.
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

/**
 * Every `/docs/…` link resolves, and every `#anchor` names a heading that is there.
 *
 * This did not exist, and its absence was not theoretical: folding the corpus site into
 * `content/docs/format/` moved twelve pages, and every cross-link between them — `/docs/conventions/…`,
 * `/docs/reading/…` — kept pointing at routes that no longer existed. `next build` prerendered all
 * 101 pages without a word. A dead internal link is invisible to the build by construction: the
 * anchor is a string, the page renders, and the reader finds out.
 *
 * The fragment half is the one that matters more, because it is the one nobody can see coming.
 * Renaming a `##` is an ordinary edit — it is prose — and five pages currently point at
 * `design/corpus#what-is-borrowed-from-graphar-and-where-borrowing-stops`. Nothing else in this
 * repository would notice that heading being reworded.
 *
 * WHAT IT CANNOT PROVE: that the target says what the link claims, which is the same residue every
 * citation guard on this site has. And it checks the slug that fumadocs derives from the heading
 * text, so a heading rewritten to different words with the same slug passes — correctly, because
 * the link still lands.
 */
const DOCS_LINK = /\]\((\/docs[^)\s]*)\)/g;

/** fumadocs' heading slug: lowercase, punctuation dropped, runs of anything else become one dash. */
function slug(heading: string): string {
  return heading
    .replace(/`/g, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .trim()
    .replace(/\s+/g, "-");
}

function anchorsOf(file: string): Set<string> {
  const body = readFileSync(file, "utf8");
  const headings = body.matchAll(/^#{2,6}\s+(.+?)\s*$/gm);
  return new Set([...headings].map((m) => slug(m[1])));
}

describe("every internal link lands", () => {
  const links = mdxUnder(CONTENT_ROOT).flatMap((file) =>
    [...readFileSync(file, "utf8").matchAll(DOCS_LINK)].map(([, href]) => ({
      id: `${relative(repoRoot, file)} → ${href}`,
      from: file,
      href,
    })),
  );

  // Same vacuity trap as everywhere else: a glob that stops matching turns this into a green pass
  // over nothing. There are ~170 of these; the floor is deliberately far below that and above zero.
  it("finds links to check", () => {
    expect(links.length).toBeGreaterThan(50);
  });

  it.each(links)("$id", ({ href }) => {
    const [route, fragment] = href.split("#");
    const target = pageFileForRoute(route);

    expect(target, `${route} does not resolve to a page under content/docs/`).not.toBeNull();
    if (!fragment) return;

    expect(
      anchorsOf(target as string),
      `${route} has no heading whose slug is #${fragment}`,
    ).toContain(fragment);
  });
});

/**
 * `arguedIn` is optional, and the reason is structural rather than lenient.
 *
 * It existed because destinations and their arguments lived in different sections — a page said
 * where it was going and pointed at the page that argued for it. Folding those together removes the
 * pointer: a page that argues its own direction in its own body has nothing to name, and forcing it
 * to name something would make it cite itself, which the assertion below rightly rejects.
 *
 * So the field survives for the case it was built for — the argument is somewhere else on this site
 * — and when it is there it is held to exactly what it was held to before.
 */
describe("every argument that is named is a page of this site", () => {
  const withArgument = pages.filter(
    (p) => (p.data.direction as { arguedIn?: string } | undefined)?.arguedIn,
  );

  it.each(withArgument)("$id names an argument on this site", ({ path, data }) => {
    const arguedIn = (data.direction as { arguedIn?: string }).arguedIn as string;

    expect(arguedIn.startsWith("/docs/"), `${arguedIn} must be a route on this site`).toBe(true);

    const target = pageFileForRoute(arguedIn);
    expect(target, `${arguedIn} does not resolve to a page under content/docs/`).not.toBeNull();
    expect(target, `${arguedIn} is the page itself, which argues nothing`).not.toBe(path);
  });
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
 * **What it does not scan:** nothing, now. This carried two exemptions and outlived both —
 * `decisions/`, and then `SURFACE-PLAN.md`, which kept the old spelling on the grounds that it had
 * an owner. Having an owner is not a property a grep can check, and the file is gone. Widening
 * `CITED_TREES` is the whole of the change if a third tree ever needs citing.
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
