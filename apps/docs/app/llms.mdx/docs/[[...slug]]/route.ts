import { notFound } from "next/navigation";
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
 */
export const revalidate = false;

export async function GET(_req: Request, { params }: { params: Promise<{ slug?: string[] }> }) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();

  // `raw` (the file off disk), not `processed`: processed Markdown would need
  // `includeProcessedMarkdown` enabled in source.config.ts.
  return new Response(await page.data.getText("raw"), {
    headers: { "content-type": "text/markdown; charset=utf-8" },
  });
}

export function generateStaticParams() {
  return source.generateParams();
}
