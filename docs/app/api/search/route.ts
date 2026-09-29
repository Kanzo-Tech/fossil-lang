import { createFromSource } from "fumadocs-core/search/server";
import { source } from "@/lib/source";

/**
 * `staticGET`, not `GET` — the index is a file, not a server. GitHub Pages runs nothing, so the
 * whole index is emitted once at build time and the browser searches it; `app/layout.tsx` tells
 * `RootProvider` `type: "static"` so it fetches this once rather than per keystroke. Setting one
 * half without the other fails silently, as a search box that returns nothing.
 *
 * The cost is bytes rather than work: the index ships to every visitor who opens search.
 */
export const revalidate = false;
export const { staticGET: GET } = createFromSource(source);
