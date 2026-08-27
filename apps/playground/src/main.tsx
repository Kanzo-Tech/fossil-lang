import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import App from './App.js';
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
