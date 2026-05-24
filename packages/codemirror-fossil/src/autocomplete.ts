/**
 * `@`-prefixed autocomplete for the Fossil editor.
 *
 * Per ADR-0029, source references take the form `@connector/path` — opaque
 * string text inside `io.csv("…")` / `io.json("…")` / `io.parquet("…")`
 * constructors in v0.1. The CodeMirror autocomplete fires whenever the
 * cursor sits after a freshly-typed `@` and lists known connector names
 * from the host-supplied {@link ConnectionResolver.list}.
 *
 * Stage layout:
 *   - Stage 1: `@`         → list ALL connectors (`resolver.list()`).
 *   - Stage 1: `@conn`     → list connectors whose name starts with `conn`.
 *   - Stage 2: `@conn/`    → would list paths inside `conn`. Returns null
 *     in v0.1 — resolvers don't yet expose `listPaths()`. Phase 9 may add it.
 *
 * Caching: `resolver.list()` results are cached for 30 seconds so the
 * autocomplete doesn't hammer the host on every keystroke. The
 * `resolver.subscribe?()` channel could invalidate this proactively;
 * v0.1 uses time-based expiry alone (matches the comment in
 * `@fossil-lang/types::ConnectionResolver` about poll-on-mount semantics).
 */

import { autocompletion } from '@codemirror/autocomplete';
import type {
  Completion,
  CompletionContext,
  CompletionResult,
  CompletionSource,
} from '@codemirror/autocomplete';
import type { Extension } from '@codemirror/state';
import type { ConnectionResolver, Connector } from '@fossil-lang/types';

/** Cache TTL for `resolver.list()` results. */
const LIST_CACHE_TTL_MS = 30_000;

/**
 * Build a CodeMirror `CompletionSource` that fires on `@`-prefixed words.
 *
 * If `resolver` is `undefined`, the source still fires on `@` (so consumers
 * see the autocomplete attempt) but returns an empty options list — this
 * keeps the UX predictable when the host hasn't wired a resolver yet
 * (e.g. a docs embed with no connectors).
 */
export function fossilAutocompleteSource(
  resolver: ConnectionResolver | undefined,
): CompletionSource {
  let cache: { connectors: Connector[]; expires: number } | null = null;

  // Subscribe to resolver invalidation events (when the channel is provided)
  // so freshly-added connectors appear immediately rather than after the
  // 30-second TTL. The optional `subscribe` returns an unsubscribe fn we
  // don't bother to call — the source is captured by the CodeMirror extension
  // for the lifetime of the editor, so the listener naturally lives that long.
  if (resolver?.subscribe) {
    resolver.subscribe(() => {
      cache = null;
    });
  }

  async function getConnectors(): Promise<Connector[]> {
    if (!resolver) return [];
    const now = Date.now();
    if (cache && cache.expires > now) return cache.connectors;
    const connectors = await resolver.list();
    cache = { connectors, expires: now + LIST_CACHE_TTL_MS };
    return connectors;
  }

  return async (context: CompletionContext): Promise<CompletionResult | null> => {
    // Match `@` followed by zero or more connector-name-compatible chars
    // (a-z, 0-9, hyphen, underscore — per ADR-0029 regex). The trailing
    // `/` (or anything after) means stage 2; we return null for that
    // until resolvers grow path enumeration.
    const word = context.matchBefore(/@[a-z0-9_\-/]*/i);
    if (!word) return null;
    // Don't auto-trigger when there's no `@` actually typed yet (the
    // `matchBefore` regex matches the empty string after the `@`).
    if (word.from === word.to && !context.explicit) return null;

    const text = word.text; // e.g. "@my-conn" or "@conn/foo"
    const slashIdx = text.indexOf('/');

    // Stage 2: path completion — not implemented in v0.1.
    if (slashIdx >= 0) return null;

    const connectorPrefix = text.slice(1); // drop leading `@`
    const connectors = await getConnectors();
    const matches = connectors.filter((c) =>
      c.name.toLowerCase().startsWith(connectorPrefix.toLowerCase()),
    );

    const options: Completion[] = matches.map((c) => ({
      // Show the connector name with a trailing `/` so accepting the
      // completion places the user one keystroke from typing a path.
      label: '@' + c.name + '/',
      detail: c.type,
      info: c.label,
      apply: '@' + c.name + '/',
      type: 'namespace',
    }));

    return { from: word.from, to: word.to, options };
  };
}

/**
 * CodeMirror `Extension` wiring {@link fossilAutocompleteSource} into the
 * editor's autocompletion engine. Pass `undefined` for `resolver` to disable
 * connector listing while keeping the autocompletion infrastructure available
 * (useful for docs embeds or read-only previews).
 */
export function fossilAutocomplete(
  resolver: ConnectionResolver | undefined,
): Extension {
  return autocompletion({
    override: [fossilAutocompleteSource(resolver)],
    activateOnTyping: true,
  });
}
