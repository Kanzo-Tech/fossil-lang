/**
 * Landing home page — a Next.js 15 Server Component that renders a thin
 * Client Component shell which in turn dynamic-imports the WASM-bearing
 * playground with `ssr: false`.
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
 *   This keeps the SSR-side HTML response tiny (just the header + the
 *   loading shell) which protects SC#3 cold-load.
 */
import { ClientShell } from './ClientShell';

export default function HomePage(): JSX.Element {
  return (
    <main
      style={{
        maxWidth: 1280,
        margin: '0 auto',
        padding: '2rem 1.5rem',
      }}
    >
      <header style={{ marginBottom: '1.5rem' }}>
        <h1 style={{ marginTop: 0, fontSize: '1.75rem', fontWeight: 600 }}>
          Fossil playground
        </h1>
        <p style={{ color: '#475569', marginTop: '0.5rem', maxWidth: 720 }}>
          Edit a Fossil mapping, type-check it in the browser, run it
          against DuckDB-WASM, and see the resulting graph. Everything
          happens client-side — no backend, works offline after first
          visit.
        </p>
      </header>
      <ClientShell />
    </main>
  );
}
