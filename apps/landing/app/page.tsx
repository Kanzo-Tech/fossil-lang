/**
 * Landing home page — a Next.js 15 Server Component that renders the
 * PLAY-06 hero above a thin Client Component shell which in turn
 * dynamic-imports the WASM-bearing playground with `ssr: false`.
 *
 * Why two boundaries (Server → ClientShell → PlaygroundHost):
 *
 *   Per Next.js 15 (verified against next@15.5.18 build error), `ssr:
 *   false` with `next/dynamic` is NO LONGER allowed inside Server
 *   Components. The fix is to put the `dynamic({ ssr: false })` call
 *   inside a Client Component (`ClientShell.tsx`) — the Server Component
 *   here only renders Server-safe markup + the Client Shell.
 *
 *   This is an EVOLUTION of RESEARCH.md Pitfall 3 (which was based on
 *   Next.js 14 — the pattern was simpler then). The structural property
 *   is the same: WASM-using code MUST live behind `ssr: false` because
 *   fossil-wasm + DuckDB-WASM + CodeMirror 6 need browser globals
 *   (`window`, `Worker`, `document`). What's changed is WHERE the
 *   `dynamic({ ssr: false })` call lives — now in a Client Component,
 *   not a Server Component.
 *
 *   This keeps the SSR-side HTML response tiny (just the hero + the
 *   loading shell) which protects SC#3 cold-load.
 *
 * PLAY-06 hero placement:
 *
 *   <LandingHero/> (Server Component, zero JS) renders ABOVE the
 *   playground. The CTA anchor links to `#playground` — the host's
 *   outer wrapper carries `id="playground"` so the browser scrolls
 *   natively (no JS handler needed). HARTIG_BIBTEX is pulled from
 *   `@fossil-lang/playground` but it's a plain string constant, so
 *   nothing else from that package ends up in this Server chunk.
 */
import { ClientShell } from './ClientShell';
import { LandingHero } from './landing-hero';

export default function HomePage(): JSX.Element {
  return (
    <>
      <LandingHero />
      {/*
        The `id="playground"` anchor pairs with `<LandingHero/>`'s CTA
        `href="#playground"` — clicking the CTA scrolls the user to the
        playground via the browser's native hash-anchor behaviour. No
        JS handler is needed; this is a Server Component.
      */}
      <main
        id="playground"
        style={{
          maxWidth: 1280,
          margin: '0 auto',
          padding: '2rem 1.5rem',
        }}
      >
        <ClientShell />
      </main>
    </>
  );
}
