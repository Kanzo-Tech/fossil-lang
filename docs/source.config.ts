import { defineDocs, defineConfig } from "fumadocs-mdx/config";
import { remarkMdxMermaid } from "fumadocs-core/mdx-plugins";
import { pageSchema } from "fumadocs-core/source/schema";
import { z } from "zod";

/**
 * The editorial stance, made a type.
 *
 * A page that describes something not yet built declares where it is going:
 *
 *   direction — where this is going, and the page on this site that argues for it
 *
 * Everything else on a page describes what is there. That asymmetry is the stance: the destination
 * is marked, the present is not, and a page with no `direction:` is making no claim about a future.
 *
 * There was a second block, `today`, carrying a one-sentence status and a `backedBy` path to a file
 * that would go red if the sentence stopped holding. It is gone, and the reason is measured rather
 * than editorial: the guard could only prove the cited file existed, never that it asserted the
 * claim, and eight of forty-nine citations had drifted to a line saying something else — one to a
 * blank line, one fifty-three lines adrift — with CI green throughout. A field that cannot fail
 * when it is wrong is not evidence; it is a citation-shaped decoration. Evidence is transclusion:
 * `<Program src= region= />` reads the file at build time and an absent region stops the build.
 *
 * fumadocs' own page schema strips unknown keys, so without this extension the block never reaches
 * `page.data` and the renderer has nothing to show. That is all this file does — it checks *shape*.
 * Presence is `content.test.ts`, which knows the path a file came from.
 */
const direction = z.object({
  summary: z.string().min(1),
  // Optional, and only because a page may argue its own direction in its own body — which is now
  // the common case, since the section that held destinations apart from their arguments folded
  // into the pages it was pointing at. When it IS present it is a route on this site, never a path
  // in the repository: an argument kept anywhere but here is a second reference, and
  // `content.test.ts` resolves it to a page that is not this one.
  arguedIn: z.string().min(1).optional(),
});

export const docs = defineDocs({
  dir: "content/docs",
  docs: {
    schema: pageSchema.extend({
      direction: direction.optional(),
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
