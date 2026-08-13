// @ts-nocheck
import * as __fd_glob_15 from "../content/docs/scale/streaming.mdx?collection=docs"
import * as __fd_glob_14 from "../content/docs/scale/larger-than-ram.mdx?collection=docs"
import * as __fd_glob_13 from "../content/docs/reading/without-fossil.mdx?collection=docs"
import * as __fd_glob_12 from "../content/docs/reading/verbs.mdx?collection=docs"
import * as __fd_glob_11 from "../content/docs/reading/mcp.mdx?collection=docs"
import * as __fd_glob_10 from "../content/docs/conventions/payload.mdx?collection=docs"
import * as __fd_glob_9 from "../content/docs/conventions/order.mdx?collection=docs"
import * as __fd_glob_8 from "../content/docs/conventions/identity.mdx?collection=docs"
import * as __fd_glob_7 from "../content/docs/conventions/adjacency.mdx?collection=docs"
import * as __fd_glob_6 from "../content/docs/conventions/addressing.mdx?collection=docs"
import * as __fd_glob_5 from "../content/docs/index.mdx?collection=docs"
import * as __fd_glob_4 from "../content/docs/guards.mdx?collection=docs"
import { default as __fd_glob_3 } from "../content/docs/scale/meta.json?collection=docs"
import { default as __fd_glob_2 } from "../content/docs/reading/meta.json?collection=docs"
import { default as __fd_glob_1 } from "../content/docs/conventions/meta.json?collection=docs"
import { default as __fd_glob_0 } from "../content/docs/meta.json?collection=docs"
import { server } from 'fumadocs-mdx/runtime/server';
import type * as Config from '../source.config';

const create = server<typeof Config, import("fumadocs-mdx/runtime/types").InternalTypeConfig & {
  DocData: {
  }
}>({"doc":{"passthroughs":["extractedReferences"]}});

export const docs = await create.docs("docs", "content/docs", {"meta.json": __fd_glob_0, "conventions/meta.json": __fd_glob_1, "reading/meta.json": __fd_glob_2, "scale/meta.json": __fd_glob_3, }, {"guards.mdx": __fd_glob_4, "index.mdx": __fd_glob_5, "conventions/addressing.mdx": __fd_glob_6, "conventions/adjacency.mdx": __fd_glob_7, "conventions/identity.mdx": __fd_glob_8, "conventions/order.mdx": __fd_glob_9, "conventions/payload.mdx": __fd_glob_10, "reading/mcp.mdx": __fd_glob_11, "reading/verbs.mdx": __fd_glob_12, "reading/without-fossil.mdx": __fd_glob_13, "scale/larger-than-ram.mdx": __fd_glob_14, "scale/streaming.mdx": __fd_glob_15, });