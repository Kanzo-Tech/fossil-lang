// @ts-nocheck
import { browser } from 'fumadocs-mdx/runtime/browser';
import type * as Config from '../source.config';

const create = browser<typeof Config, import("fumadocs-mdx/runtime/types").InternalTypeConfig & {
  DocData: {
  }
}>();
const browserCollections = {
  docs: create.doc("docs", {"guards.mdx": () => import("../content/docs/guards.mdx?collection=docs"), "index.mdx": () => import("../content/docs/index.mdx?collection=docs"), "conventions/addressing.mdx": () => import("../content/docs/conventions/addressing.mdx?collection=docs"), "conventions/adjacency.mdx": () => import("../content/docs/conventions/adjacency.mdx?collection=docs"), "conventions/identity.mdx": () => import("../content/docs/conventions/identity.mdx?collection=docs"), "conventions/order.mdx": () => import("../content/docs/conventions/order.mdx?collection=docs"), "conventions/payload.mdx": () => import("../content/docs/conventions/payload.mdx?collection=docs"), "reading/mcp.mdx": () => import("../content/docs/reading/mcp.mdx?collection=docs"), "reading/verbs.mdx": () => import("../content/docs/reading/verbs.mdx?collection=docs"), "reading/without-fossil.mdx": () => import("../content/docs/reading/without-fossil.mdx?collection=docs"), "scale/larger-than-ram.mdx": () => import("../content/docs/scale/larger-than-ram.mdx?collection=docs"), "scale/streaming.mdx": () => import("../content/docs/scale/streaming.mdx?collection=docs"), }),
};
export default browserCollections;