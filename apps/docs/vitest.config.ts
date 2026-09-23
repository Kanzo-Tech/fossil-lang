import { resolve } from "node:path";
import { defineConfig } from "vitest/config";

// One test file, and it is a guard rather than a unit test: it reads the content directory and the
// repository around it, so it runs in `node` with no DOM. It is wired ahead of `next build` in the
// package script, which is what makes a stale citation a build failure instead of stale prose.
export default defineConfig({
  resolve: {
    alias: { "@": resolve(__dirname, ".") },
  },
  test: {
    environment: "node",
    include: ["content.test.ts"],
  },
});
