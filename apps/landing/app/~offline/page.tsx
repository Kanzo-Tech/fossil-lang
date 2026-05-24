/**
 * Service Worker offline fallback page.
 *
 * Per Serwist convention, the `~offline` route is the document fallback
 * the SW serves when a navigation request misses the cache (e.g., the
 * user lands on a route they've never visited while offline).
 *
 * Phase 8 v0.1: a static informational page that distinguishes between
 * "returning user, just retry" vs "first visit, please come back online".
 * The playground itself is offline-capable AFTER first load (the SW
 * precaches WASM + bundles + bundled examples per the runtime cache
 * rules in service-worker.ts).
 *
 * This is a Server Component (no 'use client') — pure HTML, no JS
 * needed, smallest possible bundle for the offline path.
 */
export default function OfflinePage(): JSX.Element {
  return (
    <main
      style={{
        padding: '2rem',
        maxWidth: 640,
        margin: '0 auto',
        fontFamily: 'system-ui, sans-serif',
        lineHeight: 1.5,
      }}
    >
      <h1 style={{ marginTop: 0 }}>You&rsquo;re offline</h1>
      <p>
        The Fossil playground is offline-capable for the core editor +
        execution. If you&rsquo;ve visited before, the editor should load
        on retry — the Service Worker caches the WASM bundle, the language
        runtime, and the bundled hello example.
      </p>
      <p>
        If this is your first visit, please come back when you&rsquo;re
        online — the WASM bundle hasn&rsquo;t been cached yet.
      </p>
      <p>
        <a href="/">Retry</a>
      </p>
    </main>
  );
}
