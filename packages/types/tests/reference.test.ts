import { describe, expect, it } from 'vitest';

import { referenceTo } from '../src/index.js';

/**
 * `fossil_locator`'s `expand_alias`, transcribed so this file can state the round trip without wasm.
 * The real one is held in `packages/wasm/tests/reference.test.ts`, through the Rust that expands.
 */
function expand(reference: string, connections: Record<string, string>): string {
  const at = reference.startsWith('@') ? reference.indexOf('/') : -1;
  if (at < 0) return reference;
  const base = connections[reference.slice(1, at)];
  return base === undefined ? reference : `${base.replace(/\/+$/, '')}/${reference.slice(at + 1)}`;
}

const CONNECTIONS = {
  lake: 's3://lake/in/',
  deeper: 's3://lake/in/2024',
  'MinIO dev bucket': 'http://minio:9000/dev//',
  'a/b': 's3://never/',
};

describe('referenceTo', () => {
  it.each([
    ['s3://lake/in/users.csv', '@lake/users.csv'],
    ['s3://lake/in/2024/q1.csv', '@deeper/q1.csv'],
    ['s3://lake/in/2024', '@lake/2024'],
    ['http://minio:9000/dev/people/2024.csv', '@MinIO dev bucket/people/2024.csv'],
    ['s3://lake/inbox/x.csv', 's3://lake/inbox/x.csv'],
    ['s3://lake/in', 's3://lake/in'],
    ['s3://never/x.csv', 's3://never/x.csv'],
    ['/srv/data/x.csv', '/srv/data/x.csv'],
  ])('%s is written %s', (locator, reference) => {
    expect(referenceTo(locator, CONNECTIONS)).toBe(reference);
  });

  it('expands back to the locator it was given', () => {
    for (const locator of [
      's3://lake/in/users.csv',
      's3://lake/in/2024/q1.csv',
      's3://lake/in/',
      'http://minio:9000/dev/people/2024.csv',
      's3://lake/inbox/x.csv',
      's3://never/x.csv',
    ]) {
      expect(expand(referenceTo(locator, CONNECTIONS), CONNECTIONS)).toBe(locator);
    }
  });

  it('never doubles a prefix the locator already carries', () => {
    expect(referenceTo('s3://lake/in/in/x.csv', { lake: 's3://lake/in' })).toBe('@lake/in/x.csv');
  });
});
