import { llms } from "fumadocs-core/source";
import type { Node } from "fumadocs-core/page-tree";
import { source } from "@/lib/source";

/**
 * llmstxt.org-style index, generated from the live page tree.
 *
 * The tree — not `getPages()` — because the tree is where `meta.json`'s curated order lives.
 * Sorting the flat page list alphabetically discards it: `anatomy` is the third page of the book
 * and `cli` is the last, and alphabetically they arrive in the opposite order. The book is a book;
 * a machine reading a list has nothing but the order to tell it where to start. The tree also
 * carries `book/meta.json`'s `---Reference---` separator, which is the one signal that says the
 * last three pages are not more chapters.
 *
 * It also means there is no second list to keep true: a group's title and position come from its
 * own `meta.json`, so adding, renaming or reordering a page needs no edit here. `llms()` walks that
 * tree for us. The two `##` sections below it are the opposite — they say the things the page list
 * *cannot*, and they are hand-written because there is nowhere else for them to come from.
 *
 * The shape is the spec's: an H1, a blockquote summary, prose, then H2 sections whose bodies are
 * lists of `[name](url): notes`. There is deliberately no `llms-full.txt` — see the note by
 * `assertEveryPageDescribed`, and the report that came with this file.
 */

/**
 * Static, and that is load-bearing rather than an optimisation.
 *
 * kanzo-ui's version of this file derives the advertised origin from the incoming request —
 * `x-forwarded-host`, falling back to `new URL(request.url).origin` — which is careful about
 * proxies and costs it the whole guarantee below: touching `request` opts a route handler out of
 * prerendering, so its `llms.txt` is built on demand and any failure inside it is a 500 that a
 * visitor sees, at a URL nobody visits. Here the origin comes from the environment or is left out,
 * the handler takes no argument, and `next build` renders this file for real. The check below is
 * therefore a build failure, which is the only kind of failure worth writing.
 */
export const dynamic = "force-static";

/**
 * The advertised origin, or nothing.
 *
 * A deployment sets `NEXT_PUBLIC_SITE_URL`; without it the file says the paths are site-relative,
 * which is true, and does not invent a host it cannot know at build time. Guessing is the worse
 * failure: a file that confidently advertises `http://localhost:3200/docs/…` is wrong everywhere
 * it is read, and it is read by machines that will not notice.
 */
const ORIGIN = process.env.NEXT_PUBLIC_SITE_URL?.replace(/\/+$/, "");

const SUMMARY =
  "A mapping language and a corpus compiler. A fossil program says which columns of which sources become which properties of which shapes; the compiler checks that program against the shape document it produces and the data it reads, then writes a corpus a graph tool can open.";

/**
 * The one thing a machine has to know before it reads any page here, and the one thing it cannot
 * infer from a list of titles. Most of this site describes what is built; a few pages describe a
 * destination and say so in a field rather than in their tone. An agent that flattens the two
 * reports a plan as a feature, which is the exact failure the field exists to prevent.
 */
const REGISTERS =
  "**This site documents a language that is partly built, and marks the difference structurally.** A page that describes something not yet built carries a `direction` block: one sentence saying where fossil is going, plus `arguedIn`, a route to the page here that argues for it — the prior art it was read against, the measurement, and the observation that would reverse it. It is rendered above the body, and `content.test.ts` fails the build if a page declares one without an argument, or names an argument that is not a page of this site. Never report a `direction` as a description of the current implementation. Everything on a page that carries no `direction` describes what is there.";

/**
 * Abort rather than emit a page with no note.
 *
 * "Tooling for machines" is one of fossil's seven cores, so this file is a product surface and it
 * is held to the standard the rest of the site is held to: it fails loudly, at build time, in the
 * place that caused it. `llms()` is generous — a page with no `description` renders as a bare
 * `- [Title](/docs/…)` and the file still looks fine — and a bare title is precisely the entry a
 * machine cannot act on, because the whole point of the format is that the note is what lets it
 * decide whether to fetch the page. Silence here would be the same failure `<Program>` was built to
 * remove from the prose.
 *
 * The route is `force-static`, so `next build` prerenders it and this throw stops the build with
 * the offending file path in the message — verified by making one page fail on purpose:
 * `Error: llms.txt: book/anatomy.mdx has no \`description\`. …` / `exiting the build`. There is no
 * runtime path where a reader gets a half-built file instead.
 *
 * Scope is exactly what gets emitted: pages reachable from the tree. A draft that no `meta.json`
 * lists yet is not in this file and is not this file's business. Folders are not checked — a
 * `meta.json` `description` is optional in fumadocs and two of ours have none — which is a real gap
 * in the output, and one that has to be closed in the content, not here.
 */
function assertEveryPageDescribed(nodes: Node[]) {
  for (const node of nodes) {
    if (node.type === "folder") {
      assertEveryPageDescribed(node.index ? [node.index, ...node.children] : node.children);
      continue;
    }
    if (node.type !== "page") continue;

    const page = source.getNodePage(node);
    if (!page) {
      throw new Error(`llms.txt: the page tree names ${node.url}, which the loader cannot resolve.`);
    }
    if (!page.data.description?.trim()) {
      throw new Error(
        `llms.txt: ${page.path} has no \`description\`. Every page in the tree owes one — it is the note beside the link, and a link with no note is a page a machine cannot decide to read.`,
      );
    }
  }
}

/**
 * Everything the curated tree does not reach.
 *
 * fumadocs puts a page that no `meta.json` lists into `pageTree.fallback` and does not display it,
 * which today is three top-level things — including `/docs`, the front page, because the root
 * `meta.json` says `"index"` while the file is at `(root)/index.mdx` and the group folder is real
 * as far as meta resolution is concerned. Faithfulness to the curated order is this file's whole
 * point, so those pages do not get smuggled into the list above; and an index of a site that omits
 * the site's front page is not an index, so they do not get dropped either. They go here, under a
 * heading that says what happened.
 *
 * The section erases itself. Fix the names in `content/docs/meta.json` and `fallback` empties, and
 * this block emits nothing at all — there is no list here to keep true.
 */
function fallbackSection(index: ReturnType<typeof llms>): string[] {
  const orphans = source.pageTree.fallback?.children ?? [];
  if (orphans.length === 0) return [];

  assertEveryPageDescribed(orphans);

  return [
    "## Pages outside the curated order",
    "",
    "`content/docs/meta.json` does not list these, so the site's own navigation does not reach them either. They are real pages at real URLs, and this file would be lying by omission without them.",
    "",
    ...orphans.map((node) => index.indexNode(node)),
    "",
  ];
}

export function GET() {
  assertEveryPageDescribed(source.pageTree.children);
  const index = llms(source);

  const out = [
    "# fossil",
    "",
    `> ${SUMMARY}`,
    "",
    REGISTERS,
    "",
    ORIGIN
      ? `Paths below are relative to ${ORIGIN}. Append \`.mdx\` to any page URL for its Markdown source — e.g. ${ORIGIN}/docs/book/anatomy.mdx.`
      : "Paths below are site-relative: resolve them against the host this file came from. Append `.mdx` to any page URL for its Markdown source — e.g. `/docs/book/anatomy.mdx`.",
    "",
    // Worth spending four lines on, because it changes what the raw source is *for*. Everywhere
    // else, fetching the Markdown behind a rendered page gets you the same code you could have
    // scraped. Here it gets you something better: the name of the file the code actually lives in.
    "The Markdown is the better artefact on this site, and not only because it is smaller. No page contains a fossil program — a page *names* one, and the site reads it off disk while building. A `<Program src=\"shop/shop.fossil\" region=\"join\" />` in the source is a real path to a real file in this repository; the same block in the rendered HTML is a copy of it with the line numbers gone.",
    "",
    "## Pages, in reading order",
    "",
    "The order is curated in `meta.json`, not alphabetical, and it is the order to read them in. Indentation is the section; a bold entry with no link is a divider inside one.",
    "",
    ...source.pageTree.children.map((node) => index.indexNode(node)),
    "",
    ...fallbackSection(index),
    // What the page list structurally cannot say: a title tells an agent what a page is about,
    // never where the thing it describes lives.
    "## Where the things the pages describe live",
    "",
    "- `crates/` — the Rust workspace: the compiler stage by stage, the runtime, the CLI (`fossil-cli`) and the language server (`fossil-lsp`). A page that claims something is true today cites a file in here.",
    "- `apps/docs/programs/` — every program this site shows, one directory per program with its data beside it. These are the files `<Program>` transcludes; they are not extracted from the prose.",
    "- `grammar.bnf` — the language, normative, and transcluded whole into `/docs/book/grammar`. The parser implements it; it does not describe the parser.",
    "- `/docs/design/` — the arguments, and the only place they live. `direction.arguedIn` names one of these pages; `/docs/design/discarded` carries every rejected alternative with the observation that would bring it back.",
    "- `apps/docs/` — this site.",
    "",
  ];

  return new Response(out.join("\n"), {
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}
