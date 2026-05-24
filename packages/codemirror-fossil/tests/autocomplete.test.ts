/**
 * Tests for the `@`-prefixed autocomplete source.
 *
 * We drive `fossilAutocompleteSource` directly via a minimal CompletionContext
 * stub — the production code only touches `matchBefore(regex)` and `explicit`,
 * so the stub keeps the test coupling explicit. Resolvers come from
 * `@fossil-lang/resolvers::createMockResolver` so we exercise the real IoC
 * contract without standing up a network resolver.
 */
import { describe, expect, it } from 'vitest';
import { createMockResolver } from '@fossil-lang/resolvers';
import { fossilAutocompleteSource } from '../src/autocomplete.js';

/**
 * Build a minimal CompletionContext stub. `textBefore` is the document slice
 * to the left of the cursor — `matchBefore` matches the supplied regex
 * against it and returns `{from, to, text}` or null.
 */
function makeContext(textBefore: string, explicit = true) {
  return {
    matchBefore(re: RegExp) {
      const m = textBefore.match(re);
      if (!m || m[0].length === 0) {
        // Behave like CodeMirror's matchBefore: when nothing matches the
        // regex, return null (NOT a zero-length range). Tests for the no-`@`
        // case rely on this.
        return null;
      }
      return {
        from: textBefore.length - m[0].length,
        to: textBefore.length,
        text: m[0],
      };
    },
    explicit,
  };
}

describe('fossilAutocompleteSource', () => {
  it('returns null when there is no `@` before the cursor', async () => {
    const resolver = createMockResolver({
      fixtures: {},
      connectors: [{ name: 'a', type: 'examples' }],
    });
    const source = fossilAutocompleteSource(resolver);
    const result = await source(makeContext('prefix ex') as never);
    expect(result).toBeNull();
  });

  it('lists all connectors when `@` is typed alone', async () => {
    const resolver = createMockResolver({
      fixtures: {},
      connectors: [
        { name: 'examples', type: 'examples' },
        { name: 'uploads', type: 'upload' },
      ],
    });
    const source = fossilAutocompleteSource(resolver);
    const result = await source(makeContext('@') as never);
    expect(result).not.toBeNull();
    expect(result!.options.length).toBe(2);
    expect(result!.options.map((o) => o.label).sort()).toEqual([
      '@examples/',
      '@uploads/',
    ]);
  });

  it('filters connectors by typed prefix', async () => {
    const resolver = createMockResolver({
      fixtures: {},
      connectors: [
        { name: 'examples', type: 'examples' },
        { name: 'uploads', type: 'upload' },
      ],
    });
    const source = fossilAutocompleteSource(resolver);
    const result = await source(makeContext('@up') as never);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label)).toEqual(['@uploads/']);
  });

  it('returns empty options when no resolver is supplied', async () => {
    // No resolver => no connectors to list => empty options list. We
    // deliberately don't return null here because the user clearly typed
    // `@` and expects the autocomplete UI to acknowledge the trigger.
    const source = fossilAutocompleteSource(undefined);
    const result = await source(makeContext('@') as never);
    expect(result).not.toBeNull();
    expect(result!.options).toEqual([]);
  });

  it('returns null for stage-2 path completion (`@conn/…` not implemented in v0.1)', async () => {
    const resolver = createMockResolver({
      fixtures: { 'examples/hello.fossil': { url: 'blob:mocked' } },
      connectors: [{ name: 'examples', type: 'examples' }],
    });
    const source = fossilAutocompleteSource(resolver);
    const result = await source(makeContext('@examples/') as never);
    // Per the docstring on fossilAutocompleteSource: stage 2 returns null
    // so CodeMirror falls through to other completion sources or no-op.
    // Phase 9 may wire path enumeration once resolvers grow listPaths().
    expect(result).toBeNull();
  });

  it('caches resolver.list() results across calls within the TTL', async () => {
    let listCalls = 0;
    const baseResolver = createMockResolver({
      fixtures: {},
      connectors: [{ name: 'a', type: 'examples' }],
    });
    // Wrap list() to count invocations — the source should call it AT MOST
    // once across two consecutive triggers (cache hit on the second).
    const resolver = {
      ...baseResolver,
      async list() {
        listCalls++;
        return baseResolver.list();
      },
    };
    const source = fossilAutocompleteSource(resolver);
    await source(makeContext('@') as never);
    await source(makeContext('@a') as never);
    expect(listCalls).toBe(1);
  });

  it('is case-insensitive for connector-prefix filtering', async () => {
    const resolver = createMockResolver({
      fixtures: {},
      connectors: [
        { name: 'Examples', type: 'examples' },
        { name: 'uploads', type: 'upload' },
      ],
    });
    const source = fossilAutocompleteSource(resolver);
    const result = await source(makeContext('@EX') as never);
    expect(result).not.toBeNull();
    expect(result!.options.map((o) => o.label)).toEqual(['@Examples/']);
  });
});
