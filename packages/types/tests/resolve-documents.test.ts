import { describe, expect, it } from 'vitest';
import { resolveDocuments, type MissingDocument, type SourceHost } from '../src';

function workspace(names: Record<string, string[]>, roots: string[]) {
  const held = new Map<string, string>();
  return {
    held,
    missingDocuments(): MissingDocument[] {
      const named = [...roots, ...[...held.keys()].flatMap((k) => names[k] ?? [])];
      return [...new Set(named)]
        .filter((k) => !held.has(k))
        .map((key) => ({ key, locator: key.replace('@shapes/', 's3://b/') }));
    },
    registerDocument(key: string, text: string) {
      held.set(key, text);
    },
  };
}

const host = (refuse: string[] = []): SourceHost => ({
  connections: async () => ({ shapes: 's3://b' }),
  sign: async (locators) =>
    Object.fromEntries(locators.filter((l) => !refuse.includes(l)).map((l) => [l, `https://signed/${l}`])),
});

const ok = (async (url: string) => new Response(`text of ${url}`)) as typeof fetch;

describe('resolveDocuments', () => {
  it('registers under the key what it fetched from the locator', async () => {
    const ws = workspace({}, ['@shapes/shop.shex']);
    const out = await resolveDocuments(ws, host(), ok);
    expect(out).toEqual({ registered: 1, unread: [] });
    expect(ws.held.get('@shapes/shop.shex')).toBe('text of https://signed/s3://b/shop.shex');
  });

  it('follows a document that names another until nothing is missing', async () => {
    const ws = workspace({ '@shapes/a.shex': ['@shapes/b.shex'] }, ['@shapes/a.shex']);
    expect((await resolveDocuments(ws, host(), ok)).registered).toBe(2);
    expect([...ws.held.keys()]).toEqual(['@shapes/a.shex', '@shapes/b.shex']);
  });

  it('reports a locator the host will not sign, once, and stops', async () => {
    const ws = workspace({}, ['@shapes/x.shex']);
    const out = await resolveDocuments(ws, host(['s3://b/x.shex']), ok);
    expect(out.registered).toBe(0);
    expect(out.unread).toEqual([
      { key: '@shapes/x.shex', locator: 's3://b/x.shex', reason: 'the host does not sign this locator' },
    ]);
  });

  it('reports a failed fetch with its status', async () => {
    const ws = workspace({}, ['@shapes/x.shex']);
    const notFound = (async () => new Response('', { status: 404 })) as typeof fetch;
    expect((await resolveDocuments(ws, host(), notFound)).unread[0]?.reason).toBe('HTTP 404');
  });
});
