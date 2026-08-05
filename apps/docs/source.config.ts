import { defineDocs, defineConfig } from "fumadocs-mdx/config";
import { remarkMdxMermaid } from "fumadocs-core/mdx-plugins";
import { pageSchema } from "fumadocs-core/source/schema";
import { z } from "zod";

/**
 * The editorial stance, made a type.
 *
 * Every page under `characteristics/` and `protocols/` says two things and keeps them apart:
 *
 *   direction — where this is going, and the ADR that decided it
 *   today     — what is true right now, and the file that would go red if it stopped being
 *
 * fumadocs' own page schema strips unknown keys, so without this extension neither block ever
 * reaches `page.data` and the renderer has nothing to show. That is all this file does — it checks
 * *shape*, and it cannot check *presence*, because a zod schema does not know which directory the
 * file came from and the index and the decision list carry neither block by design.
 *
 * Presence is `content.test.ts`, which does know the path, and which also checks that every path a
 * page cites is still on disk. It runs ahead of `next build` in the `build` script, so both halves
 * fail the same way: the build stops.
 */
const direction = z.object({
  summary: z.string().min(1),
  decidedBy: z.string().min(1),
});

const today = z.object({
  summary: z.string().min(1),
  // A path to something on disk that would fail or change if this stopped being true. The literal
  // `unmeasured` is the honest alternative and the only one — "we think so" is not a third option.
  backedBy: z.string().min(1).optional(),
  unmeasured: z.literal(true).optional(),
});

export const docs = defineDocs({
  dir: "content/docs",
  docs: {
    schema: pageSchema.extend({
      direction: direction.optional(),
      today: today.optional(),
    }),
  },
});

export default defineConfig({
  mdxOptions: {
    // fumadocs ships this one; no third-party plugin is involved. It rewrites a ```mermaid fence
    // into `<Mermaid chart="…" />` and nothing else — the renderer is `components/mermaid.tsx`,
    // which says there why it runs in the browser rather than at build time. It has to be a remark
    // plugin because `rehypeCode` would otherwise have turned the fence into highlighted markup.
    remarkPlugins: [remarkMdxMermaid],
    rehypeCodeOptions: {
      themes: { light: "github-light", dark: "github-dark" },
      defaultColor: false,
    },
  },
});
