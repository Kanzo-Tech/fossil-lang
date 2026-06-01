import { defineConfig, mergeConfig } from "vitest/config";
import base from "../../vitest.config.base";

// Per-package overrides for @fossil-lang/introspect. The base supplies the
// happy-dom environment + v8 coverage; we only point Vitest at our tests dir.
export default mergeConfig(
  base,
  defineConfig({
    test: {
      include: ["tests/**/*.test.ts"],
    },
  }),
);
