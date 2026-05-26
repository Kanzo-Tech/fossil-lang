// Confirmed against node_modules/.pnpm/@codemirror+lsp-client@6.2.4/.../dist/index.d.ts v6.2.4:
// - LSPClient class:    new LSPClient(config?: LSPClientConfig); .connect(transport): LSPClient
// - Extension factory:  languageServerSupport(client: LSPClient, uri: string, languageID?: string): Extension
//                       (line 618 of dist/index.d.ts — returns Extension)
//
// Per ADR-0032 (lsp-client adoption) + ADR-0036 (Transport superset):
// our Transport is structurally compatible with CM6's narrower Transport
// (we only ADDED methods + optional params); pass it through directly.
import { LSPClient, languageServerSupport } from '@codemirror/lsp-client';
import type { Extension } from '@codemirror/state';
import type { Transport } from '../transports/types.js';

/**
 * Build a CM6 extension that wires the editor to an LSP server via the given
 * Transport (per ADR-0036). Used by FossilEditor's auto-composition path
 * (when the consumer omits the `extensions` prop). The playground passes
 * pre-composed extensions instead — for parity with the v0.1 LSP wiring
 * the playground composes `languageServerSupport(client, uri, 'fossil')`
 * itself; the editor's auto-composition matches that shape verbatim.
 *
 * The function name + signature match the LSP-client factory's `Extension`-
 * returning behaviour. The `'fossil'` languageID hint matches the playground's
 * carryover composition (see packages/playground/src/component/FossilPlayground.tsx
 * — `languageServerSupport(lspClient, documentUri, 'fossil')`).
 */
export function buildLspExtension(transport: Transport): Extension {
  const client = new LSPClient();
  // Structural typing: our Transport is a strict superset of CM6's narrower
  // Transport (per ADR-0036). LSPClient.connect accepts our shape directly.
  client.connect(transport);
  return languageServerSupport(client, 'file:///editor.fossil', 'fossil');
}
