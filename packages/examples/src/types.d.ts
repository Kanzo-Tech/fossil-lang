// Ambient module declaration for Vite/Rollup `?raw` imports.
//
// Vite (and Vitest, which IS Vite under the hood) resolves `import x from
// './foo.fossil?raw'` to the file contents as a string at consumer-bundle
// time. tsc has no native understanding of the `?raw` suffix, so we declare
// the module shape here. Consumers that don't use Vite (rare for this
// package) must supply equivalent typing in their own ambient declarations.
//
// This file is INCLUDED in the published package via the tsconfig `include`
// glob — downstream consumers inherit the `?raw` typing through our emitted
// `dist/*.d.ts`.

declare module '*?raw' {
  const content: string;
  export default content;
}
