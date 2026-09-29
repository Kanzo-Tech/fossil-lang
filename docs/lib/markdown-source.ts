import { notFound } from "next/navigation";
import { source } from "@/lib/source";

/** A page's Markdown source off disk, for the two `llms.mdx` routes. */
export async function markdownSource(slug: string[]): Promise<Response> {
  const page = source.getPage(slug);
  if (!page) notFound();

  // `raw` (the file off disk), not `processed`: processed Markdown would need
  // `includeProcessedMarkdown` enabled in source.config.ts.
  return new Response(await page.data.getText("raw"), {
    headers: { "content-type": "text/markdown; charset=utf-8" },
  });
}
