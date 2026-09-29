import { markdownSource } from "@/lib/markdown-source";
import { source } from "@/lib/source";

/**
 * The Markdown source of any page, at that page's own URL with `.mdx` on the end.
 *
 * `next.config.ts` rewrites `/docs/book/anatomy.mdx` here, so the suffix is the whole interface —
 * no second URL scheme for a reader to learn and nothing for `llms.txt` to enumerate. The reason it
 * is worth serving at all is `<Program>`: a page names its programs and this app reads them off
 * disk, so the rendered HTML carries fossil code that the rendered Markdown-from-HTML would give
 * back mangled. The source says `<Program src="shop/shop.fossil" region="join" />`, which is the
 * honest answer to "where does this come from" and is a shorter one.
 *
 * **The `.mdx` is part of the emitted path**, and that is what lets a static export hold it. The
 * export writes one file per route, and `/docs/design` is a page AND the parent of
 * `/docs/design/corpus`, so without the suffix it asks for `docs/design` to be a file and a
 * directory at once and dies with `EISDIR`. With it, `docs/design.mdx` is a file and `docs/design/`
 * a directory.
 * The index page, whose slug is empty, is `../../docs-index/route.ts`'s for the same reason: an
 * optional catch-all would make `docs` itself both.
 */
export const revalidate = false;

const SUFFIX = ".mdx";

export async function GET(_req: Request, { params }: { params: Promise<{ slug: string[] }> }) {
  const { slug } = await params;
  const last = slug.at(-1) ?? "";
  // Tolerated with or without the suffix: the export asks for it, a rewrite may not.
  return markdownSource(
    last.endsWith(SUFFIX) ? [...slug.slice(0, -1), last.slice(0, -SUFFIX.length)] : slug,
  );
}

export function generateStaticParams() {
  return source
    .generateParams()
    .map((p) => p.slug ?? [])
    .filter((slug) => slug.length > 0)
    .map((slug) => ({ slug: [...slug.slice(0, -1), `${slug.at(-1)}${SUFFIX}`] }));
}
