import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createDefaultResolver, parseSourceRef } from '../src/default';

// happy-dom 15 doesn't always implement URL.createObjectURL for Blob; stub
// it to a deterministic value so we can assert "starts with blob:" without
// caring about the actual UUID.
beforeEach(() => {
  globalThis.URL.createObjectURL = vi.fn().mockReturnValue('blob:mocked');
});

describe('parseSourceRef', () => {
  it('parses @connector/path correctly', () => {
    expect(parseSourceRef('@my-conn/file.parquet')).toEqual({
      raw: '@my-conn/file.parquet',
      connector: 'my-conn',
      path: 'file.parquet',
    });
  });

  it('accepts alphanumerics + hyphen + underscore', () => {
    expect(parseSourceRef('@a/x').connector).toBe('a');
    expect(parseSourceRef('@A1/x').connector).toBe('A1');
    expect(parseSourceRef('@a_b/x').connector).toBe('a_b');
    expect(parseSourceRef('@a-b-c/x').connector).toBe('a-b-c');
  });

  it('rejects invalid connector names', () => {
    expect(() => parseSourceRef('@_starts-with-underscore/x')).toThrow(
      /Invalid connector name/,
    );
    expect(() => parseSourceRef('@-starts-with-hyphen/x')).toThrow(
      /Invalid connector name/,
    );
    expect(() => parseSourceRef('@has space/x')).toThrow(/Invalid connector name/);
    expect(() => parseSourceRef('@has.dot/x')).toThrow(/Invalid connector name/);
  });

  it('rejects missing @', () => {
    expect(() => parseSourceRef('no-at/path')).toThrow(/must start with @/);
  });

  it('rejects missing /', () => {
    expect(() => parseSourceRef('@connector-only')).toThrow(/must contain \//);
  });
});

describe('createDefaultResolver — examples', () => {
  it('resolves bundled examples', async () => {
    const r = createDefaultResolver({ examples: { 'hello.csv': 'a,b\n1,2' } });
    const result = await r.resolve({
      raw: '@examples/hello.csv',
      connector: 'examples',
      path: 'hello.csv',
    });
    expect(result.url).toMatch(/^blob:/);
    expect(result.format).toBe('csv');
  });

  it('infers format from extension', async () => {
    const r = createDefaultResolver({
      examples: {
        'a.csv': 'x',
        'b.json': '{}',
        'c.parquet': new Blob([new Uint8Array([0])]),
      },
    });
    expect((await r.resolve({ raw: '@examples/a.csv', connector: 'examples', path: 'a.csv' })).format).toBe('csv');
    expect((await r.resolve({ raw: '@examples/b.json', connector: 'examples', path: 'b.json' })).format).toBe('json');
    expect((await r.resolve({ raw: '@examples/c.parquet', connector: 'examples', path: 'c.parquet' })).format).toBe('parquet');
  });

  it('throws on unknown example', async () => {
    const r = createDefaultResolver({ examples: { 'a.csv': 'x' } });
    await expect(
      r.resolve({
        raw: '@examples/missing.csv',
        connector: 'examples',
        path: 'missing.csv',
      }),
    ).rejects.toThrow(/Cannot resolve/);
  });
});

describe('createDefaultResolver — public HTTPS allowlist', () => {
  it('resolves URL on allowlist', async () => {
    const r = createDefaultResolver({ publicBuckets: ['https://data.example.com/'] });
    const result = await r.resolve({
      raw: '@public/https://data.example.com/file.parquet',
      connector: 'public',
      path: 'https://data.example.com/file.parquet',
    });
    expect(result.url).toBe('https://data.example.com/file.parquet');
    expect(result.format).toBe('parquet');
  });

  it('rejects URL off allowlist', async () => {
    const r = createDefaultResolver({ publicBuckets: ['https://data.example.com/'] });
    await expect(
      r.resolve({
        raw: '@public/https://evil.com/x.csv',
        connector: 'public',
        path: 'https://evil.com/x.csv',
      }),
    ).rejects.toThrow(/not in publicBuckets allowlist/);
  });
});

describe('createDefaultResolver — uploads', () => {
  it('resolves registered upload when allowLocalFiles=true', async () => {
    const r = createDefaultResolver({ allowLocalFiles: true });
    r.addUpload('my.csv', new Blob(['a,b\n1,2']));
    const result = await r.resolve({
      raw: '@uploads/my.csv',
      connector: 'uploads',
      path: 'my.csv',
    });
    expect(result.url).toMatch(/^blob:/);
    expect(result.format).toBe('csv');
  });

  it('throws when resolving an unknown upload path', async () => {
    const r = createDefaultResolver({ allowLocalFiles: true });
    await expect(
      r.resolve({ raw: '@uploads/missing.csv', connector: 'uploads', path: 'missing.csv' }),
    ).rejects.toThrow(/No upload registered/);
  });

  it('rejects upload when allowLocalFiles=false', () => {
    const r = createDefaultResolver({ allowLocalFiles: false });
    expect(() => r.addUpload('x.csv', new Blob([]))).toThrow(/allowLocalFiles is false/);
  });
});

describe('createDefaultResolver — list()', () => {
  it('lists only configured connectors', async () => {
    const r = createDefaultResolver({
      examples: { x: 'y' },
      allowLocalFiles: true,
    });
    const conns = await r.list();
    expect(conns.map((c) => c.name).sort()).toEqual(['examples', 'uploads']);
  });

  it('lists all three when all configured', async () => {
    const r = createDefaultResolver({
      examples: { x: 'y' },
      allowLocalFiles: true,
      publicBuckets: ['https://example.com/'],
    });
    const conns = await r.list();
    expect(conns.map((c) => c.name).sort()).toEqual(['examples', 'public', 'uploads']);
    // Types are correctly classified
    const byName = Object.fromEntries(conns.map((c) => [c.name, c.type]));
    expect(byName.examples).toBe('examples');
    expect(byName.uploads).toBe('upload');
    expect(byName.public).toBe('public_http');
  });

  it('returns empty when nothing is configured', async () => {
    const r = createDefaultResolver();
    expect(await r.list()).toEqual([]);
  });
});

describe('createDefaultResolver — fully unconfigured', () => {
  it('throws on resolve when no Tier-1 path matches', async () => {
    const r = createDefaultResolver();
    await expect(
      r.resolve({ raw: '@examples/x', connector: 'examples', path: 'x' }),
    ).rejects.toThrow(/no matching Tier-1 connector/);
  });
});
