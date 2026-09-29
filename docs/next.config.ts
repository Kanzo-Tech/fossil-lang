import { createMDX } from "fumadocs-mdx/next";
import type { NextConfig } from "next";

const withMDX = createMDX();

/**
 * Empty in development, `/fossil-lang` on GitHub Pages — one variable, read here AND by the client.
 *
 * A project page is served from `<org>.github.io/<repo>`, so every URL the app emits needs that
 * prefix. Next prefixes the ones it owns (`Link`, `redirect`, `_next/*`); it cannot prefix a string
 * handed to `fetch` or written into a bare `<a>`, and fumadocs does both — the search index and a
 * page's Markdown source. `lib/base-path.ts` reads this same variable for those two, and passing it
 * through `env` is what keeps it one spelling rather than two.
 */
const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? "";

/**
 * **The export is opt-in**, so `next dev` and `next start` keep working: `output: "export"` removes
 * the server, and with it the rewrite below. `build:static` sets it and
 * `.github/workflows/deploy-docs.yml` publishes what that builds. The cost is that the thing that
 * ships is not quite the thing `build` builds, which is why `build:static` is a script and not only
 * a workflow step.
 */
const staticExport = process.env.DOCS_STATIC_EXPORT === "1";

// No `--webpack`. The kanzo-ui docs app pins that flag because vgplot trips a temporal-dead-zone
// error under Turbopack and its charts do not mount without it; this app has no charts, no vgplot
// and no WebGL, so the flag would be cargo. Verified by building on the default bundler.
const config: NextConfig = {
  reactStrictMode: true,
  basePath,
  env: { NEXT_PUBLIC_BASE_PATH: basePath },
  // `<Program>` and the content guard both read files from the repo root, one level up. Next traces
  // the app directory by default and would warn about the reach; this says the root is the root.
  outputFileTracingRoot: new URL("..", import.meta.url).pathname,
  ...(staticExport
    ? {
        // Pages runs nothing, so every page and route handler is a file written at build time:
        // `api/search` emits its whole index (`staticGET`) and the `.mdx` source routes carry
        // `generateStaticParams`. `trailingSlash` writes `docs/design/index.html` rather
        // than `docs/design.html`, which a CDN with no rewrite rules serves at `/docs/design/`.
        output: "export" as const,
        trailingSlash: true,
      }
    : {
        // `/docs/book/anatomy.mdx` serves that page's Markdown source. Rewrites run before dynamic
        // routes, so the suffixed URL never reaches the `[[...slug]]` page. A static export has
        // nowhere to run one and Next refuses the key there; `scripts/materialise-mdx.mjs` writes
        // the same files at the same addresses after the export instead.
        async rewrites() {
          return [
            { source: "/docs.mdx", destination: "/llms.mdx/docs-index" },
            { source: "/docs/:path*.mdx", destination: "/llms.mdx/docs/:path*.mdx" },
          ];
        },
      }),
};

export default withMDX(config);
