import { markdownSource } from "@/lib/markdown-source";

/**
 * The index page's Markdown source, served at `/docs.mdx`. Its own route rather than the empty slug
 * of `../docs/[...slug]`, because a static export cannot write `llms.mdx/docs` as the index's file
 * and as every other page's directory at once. The address a reader uses does not change: a rewrite
 * reaches here on a server, and `scripts/materialise-mdx.mjs` writes the file on Pages.
 */
export const revalidate = false;

export function GET() {
  return markdownSource([]);
}
