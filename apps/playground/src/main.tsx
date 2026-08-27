import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import App from './App.js';
// `@kanzo-tech/ui` ships its stylesheet PREBUILT — a Tailwind v4 CLI pass over its own
// source plus `@kanzo-tech/theme`'s tokens, run in that repo. So Tailwind is its
// devDependency and not ours: this app has no Tailwind, no PostCSS and no config, and
// this one import is the whole of the wiring. It comes FIRST so `styles.css`'s three
// colours and font stack can override rather than be overridden.
import '@kanzo-tech/ui/styles.css';
import './styles.css';

const root = document.getElementById('root');
if (!root) throw new Error('#root is missing from index.html');

// NOT StrictMode's double-invoke by accident: `App`'s boot effect guards with a ref
// precisely because instantiating two wasm modules and two DuckDB workers on mount is the
// bug StrictMode exists to surface, and it is cheaper to guard than to discover in prod.
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
