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

interface Manifest {
  /** The `[package] name`, because `xtask`'s is not its directory's name plus a prefix. */
  name: string;
  /** The `fossil-*` keys under `[dependencies]`, sorted. Dev and build edges are not read. */
  deps: string[];
}

/** Enough of a TOML reader for two questions: who a crate is, and which siblings it links. */
function readManifest(path: string): Manifest {
  let section = "";
  let name = "";
  const deps: string[] = [];

  for (const raw of readFileSync(path, "utf8").split("\n")) {
    const line = raw.trim();
    if (line.startsWith("#")) continue;

    const header = /^\[([^\]]+)\]/.exec(line);
    if (header) {
      section = header[1];
      continue;
    }

    if (section === "package") {
      const declared = /^name\s*=\s*"([^"]+)"/.exec(line);
      if (declared) name = declared[1];
    }

    const key = /^(fossil-[a-z0-9-]+)\s*=/.exec(line);
    if (section === "dependencies" && key) deps.push(key[1]);
  }

  return { name, deps: deps.sort() };
}

function fossilDependenciesOf(manifest: string): string[] {
  return readManifest(manifest).deps;
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
 * The group diagram on `architecture.mdx` is the whole of the grouping — there is no second file
 * that files a crate under a subject — so the diagram is read as the declaration and the manifests
 * are read as the truth.
 *
 * Five figures per drawing rot independently and none of them is visible in a diff: which crates a
 * group holds, how many it says it holds, the number on each arrow, the total in the sentence
 * below, and whether the whole thing is still a DAG. Every one of them was hand-counted before this
 * guard existed, and hand-counting is how `xtask` gets mapped as `fossil-xtask` and one edge
 * vanishes — 46 where the tree has 47.
 *
 * The page's `direction:` is *acyclic*, and it is one edge away. That last edge is named here
 * rather than tolerated: remove `fossil-layout → fossil-df` and the group graph must have no cycle
 * left at all, which makes the direction's closure condition a test rather than a promise.
 *
 * What this does NOT prove:
 *
 *   - **That the grouping is right.** Nothing can. It proves the drawing matches the tree, and a
 *     crate filed under the wrong subject is a drawing that matches the tree perfectly.
 *   - **Anything about a dev or build edge.** Same scope as the page: `[dependencies]` only.
 *   - **That the excuse is good.** It pins the one excused edge in place, so deleting the excuse
 *     without deleting the edge fails, and deleting the edge without rewriting the page fails too.
 */
const ARCHITECTURE = join(CONTENT_ROOT, "(root)/architecture.mdx");

/**
 * The one cross-group edge the page keeps, with its reason written beside it there: moving
 * `files::batches_to_parquet` would put `arrow` + `parquet` into a browser bundle that has neither.
 */
const EXCUSED_EDGE: readonly [string, string] = ["fossil-layout", "fossil-df"];

const crates: Manifest[] = readdirSync(join(repoRoot, "crates"))
  .map((dir) => join(repoRoot, "crates", dir, "Cargo.toml"))
  .filter(existsSync)
  .map(readManifest);

interface GroupNode {
  /** The mermaid node id, which is what the arrows use. */
  id: string;
  name: string;
  /** The count the label claims, kept apart from the list so the two can disagree. */
  claimed: number;
  members: string[];
}

/** The first mermaid fence of the page: the group diagram. The later ones draw crates. */
function groupDiagram(): { nodes: GroupNode[]; arrows: Map<string, number> } {
  const fence = /```mermaid\n([\s\S]*?)```/.exec(readFileSync(ARCHITECTURE, "utf8"))?.[1] ?? "";
  const names = new Set(crates.map((c) => c.name));

  const nodes: GroupNode[] = [];
  for (const [, id, label] of fence.matchAll(/^\s*(\w+)\["([^"]+)"\]\s*$/gm)) {
    const parts = /^<b>([^<]+)<\/b> · (\d+) crates<br\/>(.+)$/.exec(label);
    if (!parts) continue;
    nodes.push({
      id,
      name: parts[1],
      claimed: Number(parts[2]),
      // A crate is written without its prefix, except the one that has none.
      members: parts[3]
        .split(/<br\/>|·/)
        .map((s) => s.trim())
        .filter(Boolean)
        .map((short) => (names.has(short) ? short : `fossil-${short}`)),
    });
  }

  const arrows = new Map<string, number>();
  for (const [, from, count, to] of fence.matchAll(/^\s*(\w+)\s*-->\|(\d+)\|\s*(\w+)\s*$/gm)) {
    arrows.set(`${from} -> ${to}`, Number(count));
  }

  return { nodes, arrows };
}

const { nodes: groups, arrows: declaredArrows } = groupDiagram();

/** Which group each crate is drawn in. A crate nobody drew is absent, not defaulted. */
const groupOf = new Map<string, string>(
  groups.flatMap((g) => g.members.map((m) => [m, g.id] as const)),
);

/** Every cross-group `[dependencies]` edge, counted per ordered pair of groups. */
function measuredArrows(skip: readonly (readonly [string, string])[] = []): Map<string, number> {
  const out = new Map<string, number>();

  for (const crate of crates) {
    for (const dep of crate.deps) {
      if (skip.some(([from, to]) => from === crate.name && to === dep)) continue;
      const from = groupOf.get(crate.name);
      const to = groupOf.get(dep);
      if (!from || !to || from === to) continue;
      const key = `${from} -> ${to}`;
      out.set(key, (out.get(key) ?? 0) + 1);
    }
  }

  return out;
}

function rendered(arrows: Map<string, number>): string[] {
  return [...arrows].map(([edge, n]) => `${edge}: ${n}`).sort();
}

/** The first cycle over the group graph, named, or `undefined` if there is none. */
function cycle(arrows: Map<string, number>): string | undefined {
  const adjacent = new Map<string, string[]>();
  for (const edge of arrows.keys()) {
    const [from, to] = edge.split(" -> ");
    adjacent.set(from, [...(adjacent.get(from) ?? []), to]);
  }

  const done = new Set<string>();
  const walk = (node: string, path: string[]): string | undefined => {
    if (path.includes(node)) return [...path.slice(path.indexOf(node)), node].join(" -> ");
    if (done.has(node)) return undefined;
    for (const next of adjacent.get(node) ?? []) {
      const found = walk(next, [...path, node]);
      if (found) return found;
    }
    done.add(node);
    return undefined;
  };

  for (const node of adjacent.keys()) {
    const found = walk(node, []);
    if (found) return found;
  }
  return undefined;
}

describe("the group diagram is the manifests", () => {
  it("finds a diagram, and crates to check it against", () => {
    expect(groups.length, "no group node parsed out of the first mermaid fence").toBeGreaterThan(1);
    expect(declaredArrows.size, "no labelled arrow parsed").toBeGreaterThan(1);
    expect(crates.length, "no crate manifest read").toBeGreaterThan(20);
  });

  it("files every crate exactly once", () => {
    const filed = groups.flatMap((g) => g.members);
    expect([...filed].sort()).toEqual(crates.map((c) => c.name).sort());
  });

  it.each(groups)("$name lists as many crates as it claims", ({ claimed, members }) => {
    expect(members.length).toBe(claimed);
  });

  it("labels every arrow with the number of edges the manifests carry", () => {
    expect(rendered(declaredArrows)).toEqual(rendered(measuredArrows()));
  });

  it("states the total the arrows add up to", () => {
    const stated = /\*\*(\d+) cross-group edges\*\*/.exec(readFileSync(ARCHITECTURE, "utf8"));
    expect(stated, "the page no longer states a cross-group total").not.toBeNull();
    expect(Number(stated?.[1])).toBe([...declaredArrows.values()].reduce((a, b) => a + b, 0));
  });

  it("keeps the one edge the page excuses", () => {
    const [from, to] = EXCUSED_EDGE;
    expect(
      crates.find((c) => c.name === from)?.deps,
      `${from} no longer depends on ${to}: the direction may be closed, and the page has to say so`,
    ).toContain(to);
  });

  it("is acyclic once that edge is removed", () => {
    expect(cycle(measuredArrows([EXCUSED_EDGE]))).toBeUndefined();
  });
});

/**
 * A source citation names an ITEM, never a line. Same rule as `grammar.bnf` below, same reason.
 *
 * **What was here: `` `path/to/file.ext:12` `` had to name a line that exists.** It caught three
 * real errors the day it was written — a range past the end of a file, a mistyped path — and then it
 * was measured over the whole tree, which is the part that decided this. Of **35 citations, 13 did
 * not say what the page claimed**: 37%, with CI green on every one of them, because the assertion
 * was `last <= lines` and nothing else.
 *
 * The thirteen were not typos. They are what a line number does on its own:
 *
 *   - `fossil-df/src/stdlib.rs:78` was a **blank line**, cited as where a list of "thirteen"
 *     functions is pinned. The list is at :103 and holds four. The page contradicted itself six
 *     sections apart and the guard could not see either half.
 *   - `fossil-hir/src/lower.rs:3207` was cited for a test the prose NAMES in the sentence before it.
 *     The name is at :3347; :3207 is a different test's fixture. The citation and the name in the
 *     prose disagreed, in the same sentence, and only the unreadable half was checked.
 *   - Four cited the `use` at the top of a file for a claim about the function underneath —
 *     `system.rs:23` for `System::descriptors` at :55, `completion.rs:63` for `completions` at :80.
 *   - Three had simply slid: `lower.rs:304` for `HirExpr` at :334, `stdlib.rs:311` for `SigTy` at
 *     :451, `df/lib.rs:1792` for `vertex_info` at :1858.
 *
 * A line number is an offset into a file that is edited by definition. It is invalidated by an
 * insertion anywhere above it and the failure is SILENT: the citation still resolves, to different
 * text. Nothing about that is specific to `grammar.bnf`, which is why the spelling is now the one
 * that file already uses — `` `crates/…/x.rs, item_name` `` — and the old one is banned rather than
 * deprecated, because an accepted second spelling is how the first one comes back.
 *
 * **This is not the guard `apps/corpus/CLAUDE.md` forbids rebuilding.** That one proves a cited line
 * is on disk and implies it says something; 159 dead references accumulated under it. An anchor is a
 * NAME, so reordering a file, inserting an item, or rewriting every comment in it cannot make a
 * citation point somewhere else. It either names something the file defines or it does not.
 *
 * **What it still cannot prove**, and the residue is the same one the grammar guard admits:
 *
 *   - **That the item says what the citation claims.** `lower.rs, HirExpr` beside a sentence about
 *     the checker passes here. What changed is that a wrong anchor is now a wrong NAME — legible to
 *     a reader who knows the tree — instead of a number nobody can evaluate by eye.
 *   - **That a citation should have been there at all.** Where the claim is «this is pinned in that
 *     file» and no single item carries it, the conversion wrote the bare path and no anchor. Nothing
 *     checks a bare path: see `design/discarded`, which records why, and what would change it.
 */
const RUST_CITATION = /`(crates\/[\w./-]+\.rs),\s*([A-Za-z_][A-Za-z0-9_]*)`/g;

/**
 * The banned spelling: any backticked path with a line number after it.
 *
 * Wider than the anchor form on purpose — it covers every extension the old guard read, so that
 * converting `.rs` cannot leave `packages/…/x.ts:12` as a legal unchecked form beside it. The
 * optional backtick before the colon is not decoration; `` `grammar.bnf`:462 `` was in the tree, and
 * a pattern anchored on the bare filename walked straight past it.
 */
const LINE_CITATION = /`?[\w./-]+\.(?:rs|toml|bnf|mjs|ts|tsx|yml|json)`?:\d+(?:-\d+)?`?/;

/**
 * Every item a Rust file DEFINES: `fn`, `struct`, `enum`, `trait`, `union`, `type`, `mod`, `const`,
 * `static`, and `macro_rules!`.
 *
 * A name only MENTIONED — in a `use`, in a call, in a doc comment — is not defined, and that is the
 * whole of the improvement rather than an oversight. Four of the thirteen cited an import for a
 * claim about the item it imports; under this rule those cite the item, and the file that merely
 * names it cannot stand in for the file that has it.
 */
const RUST_DEF =
  /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+|async\s+|unsafe\s+|extern\s+"[^"]*"\s+)*(?:fn|struct|enum|trait|union|type|mod|static|const)\s+([A-Za-z_][A-Za-z0-9_]*)|^\s*macro_rules!\s+([A-Za-z_][A-Za-z0-9_]*)/;

function rustItemsOf(path: string): Set<string> {
  const full = join(repoRoot, path);
  if (!existsSync(full)) return new Set();
  return new Set(
    readFileSync(full, "utf8").split("\n").flatMap((line) => {
      const m = RUST_DEF.exec(line);
      return m ? [(m[1] ?? m[2]) as string] : [];
    }),
  );
}

interface RustCitation {
  /** `<page>:<line in the page>` — so a failure message points at the prose, not the target. */
  where: string;
  path: string;
  anchor: string;
}

const contentPages = mdxUnder(CONTENT_ROOT);

const rustCitations: RustCitation[] = contentPages.flatMap((file) => {
  const page = relative(repoRoot, file);
  return readFileSync(file, "utf8").split("\n").flatMap((line, index) =>
    [...line.matchAll(RUST_CITATION)].map((m) => ({
      where: `${page}:${index + 1}`,
      path: m[1],
      anchor: m[2],
    })),
  );
});

describe("every source citation names an item that exists", () => {
  // Without these two a regex that stopped matching would report a clean sweep of nothing.
  it("finds citations at all", () => {
    expect(rustCitations.length).toBeGreaterThan(20);
  });

  it("reads items out of a cited file", () => {
    expect(rustItemsOf("crates/fossil-hir/src/lower.rs").has("HirExpr")).toBe(true);
  });

  it.each(contentPages.map((file) => ({ id: relative(repoRoot, file), file })))(
    "$id cites no line numbers",
    ({ file }) => {
      const offender = readFileSync(file, "utf8")
        .split("\n")
        .findIndex((line) => LINE_CITATION.test(line));
      expect(
        offender,
        `line ${offender + 1} cites a file by line number, a spelling this guard cannot check; ` +
          "write `path/to/file.rs, item_name`, transclude it, or name the file with no line",
      ).toBe(-1);
    },
  );

  it.each(rustCitations)("$where cites $path, $anchor", ({ path, anchor }) => {
    expect(existsSync(join(repoRoot, path)), `${path} is not on disk`).toBe(true);
    const items = rustItemsOf(path);
    expect(items.has(anchor), `${path} defines no ${anchor}`).toBe(true);
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
