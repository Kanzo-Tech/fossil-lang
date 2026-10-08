import { describe, expect, it } from 'vitest';

import { referenceTo } from '../src/index.js';

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
  ])('%s is written %s', (location, reference) => {
    expect(referenceTo(location, CONNECTIONS)).toBe(reference);
  });

  it('never doubles a prefix the location already carries', () => {
    expect(referenceTo('s3://lake/in/in/x.csv', { lake: 's3://lake/in' })).toBe('@lake/in/x.csv');
  });
});
