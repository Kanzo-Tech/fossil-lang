/**
 * The project-page prefix, for the URLs Next cannot prefix itself: a string handed to `fetch` or
 * written into a bare `<a>`. Empty everywhere but GitHub Pages; `next.config.ts` owns the value.
 */
export const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? "";
