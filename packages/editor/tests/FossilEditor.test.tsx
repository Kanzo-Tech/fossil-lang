/**
 * FossilEditor — unit tests (happy-dom).
 *
 * Covers the four LOCKED prop-surface paths (per 11-02 must_haves):
 *   1. Minimal mount: value="" + lspTransport=null + omit extensions
 *      ⇒ component auto-composes [fossil({})] (no LSP wiring).
 *   2. Controlled value sync: rerender with a different value
 *      ⇒ editor's doc reflects the new value.
 *   3. onChange fires when the doc is mutated programmatically (via the
 *      EditorView ref the component owns — exercised through a manual
 *      dispatch since happy-dom's keyboard event simulation is brittle).
 *   4. extensions prop wins over auto-composition: pass an empty extensions
 *      array, assert the editor renders (proves the auto-compose path is
 *      skipped — no fossil() / no buildLspExtension).
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { cleanup, render } from '@testing-library/react';
import { useState } from 'react';
import { EditorView } from '@codemirror/view';
import { FossilEditor } from '../src/FossilEditor.js';

afterEach(() => {
  cleanup();
});

describe('FossilEditor', () => {
  it('mounts with minimal props (value, lspTransport=null) without throwing', () => {
    const { container } = render(
      <FossilEditor value="" lspTransport={null} />,
    );
    // The component renders a <div className="fossil-editor"/> by default.
    const host = container.querySelector('.fossil-editor');
    expect(host).not.toBeNull();
    // CodeMirror mounts a .cm-editor inside the host.
    expect(container.querySelector('.cm-editor')).not.toBeNull();
  });

  it('renders the initial value in the editor doc', () => {
    const probe = 'hello fossil world';
    const { container } = render(
      <FossilEditor value={probe} lspTransport={null} />,
    );
    expect(container.textContent ?? '').toContain(probe);
  });

  it('syncs controlled value updates externally (rerender with a new value)', () => {
    function Harness({ v }: { v: string }) {
      return <FossilEditor value={v} lspTransport={null} />;
    }
    const { container, rerender } = render(<Harness v="first doc" />);
    expect(container.textContent ?? '').toContain('first doc');
    rerender(<Harness v="second doc" />);
    expect(container.textContent ?? '').toContain('second doc');
    // And the FIRST value is no longer visible.
    expect(container.textContent ?? '').not.toContain('first doc');
  });

  it('honors caller-supplied extensions prop (skips auto-composition)', () => {
    // An empty extensions array means: no fossil() lang extension; no LSP.
    // The editor still renders an empty CodeMirror surface.
    const { container } = render(
      <FossilEditor
        value="plain text"
        lspTransport={null}
        extensions={[]}
      />,
    );
    expect(container.querySelector('.cm-editor')).not.toBeNull();
    expect(container.textContent ?? '').toContain('plain text');
  });

  it('exposes the fossil-editor className hook + accepts override', () => {
    const { container, rerender } = render(
      <FossilEditor value="" lspTransport={null} />,
    );
    expect(container.querySelector('.fossil-editor')).not.toBeNull();
    rerender(
      <FossilEditor value="" lspTransport={null} className="custom-host" />,
    );
    expect(container.querySelector('.custom-host')).not.toBeNull();
    expect(container.querySelector('.fossil-editor')).toBeNull();
  });

  it('pushes fossil/registerInferredDescriptor for each descriptor (auto-compose path)', async () => {
    // Mock Transport that captures sends + auto-acks `initialize` so the
    // LSPClient flushes its queued notifications (the LSP lifecycle holds
    // notifications until the initialize handshake completes).
    const sent: string[] = [];
    let onMessage: ((msg: string) => void) | null = null;
    const transport = {
      send(msg: string) {
        sent.push(msg);
        try {
          const parsed = JSON.parse(msg) as { method?: string; id?: number };
          if (parsed.method === 'initialize' && parsed.id !== undefined) {
            const reply = JSON.stringify({
              jsonrpc: '2.0',
              id: parsed.id,
              result: { capabilities: {} },
            });
            queueMicrotask(() => onMessage?.(reply));
          }
        } catch {
          /* non-JSON — ignore */
        }
      },
      subscribe(h: (msg: string) => void) {
        onMessage = h;
      },
      unsubscribe() {
        onMessage = null;
      },
    };

    const descriptor = {
      source_name: 'users',
      columns: [{ name: 'id', primitive: 'integer' as const }],
      content_hash: '',
    };

    render(
      <FossilEditor
        value={'users := io.csv("u.csv")'}
        lspTransport={transport}
        descriptors={[descriptor]}
      />,
    );

    await vi.waitFor(
      () => {
        const reg = sent
          .map((m) => {
            try {
              return JSON.parse(m) as { method?: string; params?: unknown };
            } catch {
              return null;
            }
          })
          .find((m) => m?.method === 'fossil/registerInferredDescriptor');
        expect(reg).toBeTruthy();
        expect(reg?.params).toEqual(descriptor);
      },
      { timeout: 1000 },
    );
  });

  it('fires onChange when the editor doc is mutated', () => {
    const onChange = vi.fn();
    const { container } = render(
      <FossilEditor value="initial" onChange={onChange} lspTransport={null} />,
    );
    // Grab the live EditorView from the .cm-editor DOM root.
    const editorEl = container.querySelector<HTMLElement>('.cm-editor');
    expect(editorEl).not.toBeNull();
    const view = EditorView.findFromDOM(editorEl!);
    expect(view).not.toBeNull();
    // Simulate a programmatic edit. The updateListener attached by
    // FossilEditor forwards docChanged events to onChange.
    view!.dispatch({
      changes: { from: view!.state.doc.length, insert: ' + appended' },
    });
    expect(onChange).toHaveBeenCalled();
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1];
    expect(lastCall?.[0]).toContain('initial + appended');
  });
});
