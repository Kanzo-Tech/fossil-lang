import { defineDocs, defineConfig } from "fumadocs-mdx/config";
import { remarkMdxMermaid } from "fumadocs-core/mdx-plugins";

/**
 * No frontmatter extension, and that is a difference from the language site worth stating rather
 * than inheriting by accident.
 *
 * That site documents a destination, so every page there carries two registers — where this is
 * going, and what is true right now — and a zod schema keeps them apart. This site documents a
 * **format**. A specification has one tense. A corpus either satisfies a convention or it does not,
 * and the thing that says which is not a frontmatter field: it is `guards/check.mjs`, run against
 * the corpus.
 *
 * **And there is no guard over the prose here, on purpose.** The obvious one — assert that every
 * `file:line` a page cites is on disk — was tried next door and audited: it checks that the line
 * *exists*, never that it *says* what the page claims, and 159 dead references accumulated
 * underneath it with CI green the whole time, one of them pointing at a blank line. What replaces it
 * is a guard that measures the artifact instead of the page, and two server components that render
 * from the source of truth rather than transcribing it: `components/guard-index.tsx` reads the
 * guards themselves, `components/vector-table.tsx` reads the vectors the checker executes. A page
 * cannot drift from a table it does not contain.
 */
export const docs = defineDocs({ dir: "content/docs" });

export default defineConfig({
  mdxOptions: {
    remarkPlugins: [remarkMdxMermaid],
    rehypeCodeOptions: {
      themes: { light: "github-light", dark: "github-dark" },
      defaultColor: false,
    },
  },
});
