import { createMDX } from "fumadocs-mdx/next";
import type { NextConfig } from "next";

const withMDX = createMDX();

const config: NextConfig = {
  reactStrictMode: true,
  // Two server components reach outside this app: the guard index reads `guards/guards.mjs` so the
  // contract page cannot disagree with the code it documents, and the vector table reads
  // `guards/vectors.json` so the published borders are the executed ones. Next traces the app
  // directory by default and would warn about the reach; this says the root is the root.
  outputFileTracingRoot: new URL("../..", import.meta.url).pathname,
};

export default withMDX(config);
