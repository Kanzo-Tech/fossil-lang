/**
 * Root layout for the Fossil playground landing host.
 *
 * Phase 8 v0.1 ships a minimal shell — the playground component carries
 * its own chrome (toolbar + editor + results regions per A11Y-01 from
 * 08-10). The landing page is intentionally bare so the PRODUCT is the
 * playground itself, not surrounding marketing copy. Phase 9 PLAY-06
 * adds branding + cite-snippet + nav via `<LandingHero/>` above the
 * playground, with styles in `./styles/landing.css`.
 *
 * The CSS import goes at the very top of the root layout so the
 * cascade is set before any page renders. Next.js's App Router
 * supports plain `.css` imports here — they're emitted as static
 * stylesheets and referenced from the <head> automatically.
 */
import './styles/landing.css';
import type { Metadata, Viewport } from 'next';

export const metadata: Metadata = {
  title: 'Fossil — Typed mapping playground',
  description:
    'In-browser typed RDF graph construction with Fossil. Edit. Type-check. Run. See triples — without leaving the browser.',
  applicationName: 'Fossil playground',
  // Manifest is emitted by Serwist alongside the SW; lock the path.
  manifest: '/manifest.webmanifest',
};

export const viewport: Viewport = {
  width: 'device-width',
  initialScale: 1,
  themeColor: '#0f172a',
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}): JSX.Element {
  return (
    <html lang="en">
      <body
        style={{
          margin: 0,
          fontFamily:
            'system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
          minHeight: '100vh',
          background: '#ffffff',
          color: '#0f172a',
        }}
      >
        {children}
      </body>
    </html>
  );
}
