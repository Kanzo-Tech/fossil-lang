import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createDefaultResolver, createMockResolver } from '../src/index';
import type { ConnectionResolver } from '@fossil-lang/types';

// happy-dom may not implement URL.createObjectURL deterministically; stub
// it so the resolve flow under test completes without env-specific failure.
beforeEach(() => {
  globalThis.URL.createObjectURL = vi.fn().mockReturnValue('blob:mocked');
});

/**
 * Patterns that look like credentials. If a resolver's exposed surface ever
 * contains a string matching any of these AFTER a normal resolve flow,
 * CONN-01 is violated.
 *
 * This is the structural proof of the resolver invariant: the component
 * NEVER sees plaintext credentials. Even when a host passes a presigned
 * URL containing credential-shape query params, the resolver MUST treat
 * it as ephemeral — return it in the ResolvedSource and forget it; never
 * cache it in a property a downstream consumer can read back via the
 * exposed surface.
 */
const CREDENTIAL_PATTERNS: RegExp[] = [
  // AWS access key id (16-uppercase-alnum starting with AKIA)
  /AKIA[0-9A-Z]{16}/,
  // JWT (three base64url segments separated by dots; entropy thresholds
  // chosen to avoid matching short doc-style "abc.def.ghi" placeholders)
  /eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{10,}/,
  // Authorization Bearer / Basic literal prefixes
  /Bearer\s+[A-Za-z0-9._-]{20,}/i,
  /Basic\s+[A-Za-z0-9+/=]{20,}/i,
  // key=value pairs — password, token, secret, api_key with substantial RHS
  /(?:password|token|secret|api[_-]?key)\s*[:=]\s*["']?[^\s"']{8,}/i,
];

/** Deep-walk an object and collect every string it transitively contains. */
function collectStrings(obj: unknown, acc: string[] = [], depth = 0): string[] {
  if (depth > 10) return acc; // cycle guard
  if (typeof obj === 'string') {
    acc.push(obj);
  } else if (obj && typeof obj === 'object') {
    for (const v of Object.values(obj as Record<string, unknown>)) {
      collectStrings(v, acc, depth + 1);
    }
  }
  return acc;
}

/** Throw if any string transitively reachable from `resolver` matches a credential pattern. */
function assertNoCredentialsLeak(
  resolver: ConnectionResolver,
  label: string,
): void {
  const strings = collectStrings(resolver);
  for (const s of strings) {
    for (const pattern of CREDENTIAL_PATTERNS) {
      if (pattern.test(s)) {
        throw new Error(
          `${label}: credential-shape string leaked into resolver state: ${s.slice(0, 50)}... (pattern: ${pattern})`,
        );
      }
    }
  }
}

describe('CONN-01: no credentials in resolver state', () => {
  it('default resolver does not store credentials even when a presigned URL is resolved', async () => {
    const r = createDefaultResolver({
      publicBuckets: ['https://s3.amazonaws.com/'],
    });
    // Simulate a host passing a presigned URL with credential-shape query params
    await r.resolve({
      raw: '@public/https://s3.amazonaws.com/bucket/key?X-Amz-Credential=AKIAEXAMPLE123456789',
      connector: 'public',
      path: 'https://s3.amazonaws.com/bucket/key?X-Amz-Credential=AKIAEXAMPLE123456789',
    });
    // After resolution, the resolver itself must not retain the URL in any
    // state reachable via the public surface (resolve/list/etc.).
    expect(() => assertNoCredentialsLeak(r, 'default resolver')).not.toThrow();
  });

  it('default resolver does not retain upload paths or example contents after resolve', async () => {
    const r = createDefaultResolver({
      examples: { 'hello.csv': 'a,b\n1,2' },
      allowLocalFiles: true,
    });
    r.addUpload('my.csv', new Blob(['a,b\n1,2']));
    await r.resolve({
      raw: '@examples/hello.csv',
      connector: 'examples',
      path: 'hello.csv',
    });
    // Public surface should not surface credentials. The internal Map and
    // opts.examples object are module-private (closure-captured); not
    // reachable via Object.values walking the handle.
    expect(() => assertNoCredentialsLeak(r, 'default resolver post-upload')).not.toThrow();
  });

  it('mock resolver with public-only fixtures does not match credential patterns', () => {
    const r = createMockResolver({
      fixtures: {
        'examples/hello.csv': { url: 'blob:abc', format: 'csv' },
        'examples/users.parquet': {
          url: 'https://public.example.com/users.parquet',
          format: 'parquet',
        },
      },
    });
    expect(() => assertNoCredentialsLeak(r, 'mock resolver')).not.toThrow();
  });

  // Positive controls — these exercise the leak detector against a HYPOTHETICAL
  // bad resolver that would leak. We can't reach the closure-captured fixtures
  // of createMockResolver via Object.values (which is exactly the property that
  // makes the actual resolvers safe — module-private state is unreachable), so
  // the positive control simulates a leaky resolver by exposing the bad strings
  // as enumerable own properties on a ConnectionResolver-shaped object. If the
  // leak detector's CREDENTIAL_PATTERNS regex ever degrades, these tests stop
  // throwing — that's the contract.
  it('positive control: leak detector flags secret= key=value pairs', () => {
    const leakyResolver = {
      resolve: async () => ({ url: 'x' }),
      list: async () => [],
      // simulate a buggy resolver that exposed its fixtures as a public prop:
      __leakedFixture: 'https://x.com/?secret=hunter2hunter2hunter2',
    } as unknown as ConnectionResolver;
    expect(() => assertNoCredentialsLeak(leakyResolver, 'leaky resolver')).toThrow(
      /credential-shape string leaked/,
    );
  });

  it('positive control: leak detector flags AWS-key-shaped strings', () => {
    const leakyResolver = {
      resolve: async () => ({ url: 'x' }),
      list: async () => [],
      __leakedKey: 'https://x.com/?AccessKey=AKIAIOSFODNN7EXAMPLE',
    } as unknown as ConnectionResolver;
    expect(() => assertNoCredentialsLeak(leakyResolver, 'leaky AWS-key resolver')).toThrow(
      /credential-shape string leaked/,
    );
  });

  it('positive control: leak detector flags Bearer-token-shaped strings', () => {
    const leakyResolver = {
      resolve: async () => ({ url: 'x' }),
      list: async () => [],
      __leakedAuth: 'Bearer abc123def456ghi789jkl012mno345',
    } as unknown as ConnectionResolver;
    expect(() => assertNoCredentialsLeak(leakyResolver, 'leaky bearer resolver')).toThrow(
      /credential-shape string leaked/,
    );
  });

  it('CONN-01 architectural property: createMockResolver fixtures are closure-private (unreachable via Object walk)', () => {
    // This test makes the safety property of the actual resolver factories
    // EXPLICIT: closure capture is what makes them safe. createMockResolver's
    // returned object only exposes resolve+list methods; opts.fixtures is
    // captured in the closure but not reachable via Object.values walk.
    const r = createMockResolver({
      fixtures: {
        // even with credential-shape strings here, they DON'T leak via the
        // public surface — the closure hides them from Object.values
        'bad/key': {
          url: 'https://x.com/?secret=hunter2hunter2hunter2',
          format: 'csv',
        },
      },
    });
    // Because the fixtures are closure-private, the walker sees ONLY the
    // resolve+list functions (which stringify to function source, not
    // matching credential patterns). This is by design — the load-bearing
    // property that makes ConnectionResolver implementations safe.
    expect(() => assertNoCredentialsLeak(r, 'safe-by-closure mock resolver')).not.toThrow();
  });
});
