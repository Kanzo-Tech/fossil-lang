import { createMDX } from "fumadocs-mdx/next";
import type { NextConfig } from "next";

const withMDX = createMDX();

// No `--webpack`. The kanzo-ui docs app pins that flag because vgplot trips a temporal-dead-zone
// error under Turbopack and its charts do not mount without it; this app has no charts, no vgplot
// and no WebGL, so the flag would be cargo. Verified by building on the default bundler.
const config: NextConfig = {
  reactStrictMode: true,
  // `<Program>` and the content guard both read files from the repo root, two levels up. Next traces
  // the app directory by default and would warn about the reach; this says the root is the root.
  outputFileTracingRoot: new URL("../..", import.meta.url).pathname,
  // `/docs/book/anatomy.mdx` serves that page's Markdown source. Rewrites run before dynamic
  // routes, so the suffixed URL never reaches the `[[...slug]]` page.
  async rewrites() {
    return [
      { source: "/docs.mdx", destination: "/llms.mdx/docs" },
      { source: "/docs/:path*.mdx", destination: "/llms.mdx/docs/:path*" },
    ];
  },
};

export default withMDX(config);
