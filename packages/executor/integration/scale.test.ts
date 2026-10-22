/**
 * The shop program at the scale that used to trap: 200,000 people and 600,000 orders.
 *
 * `DataFusion`'s default pool summed reservations into a `usize` it never checked, and the sort
 * above `Order`'s dedup charged each of its seventy-three slices for the whole batch it was cut
 * from — 5.6 GiB accounted for a 78 MiB batch. On `wasm32` that sum wrapped and a reservation
 * split unwrapped a `None`: `unreachable`, mid-run, with the promise never settled. 60,000 people
 * ran. `crates/fossil-exec/src/memory.rs` is the fix and says why.
 *
 * The run must finish, every order must be written, and the executor's linear memory must stay
 * under a gigabyte — it measured 624 MiB when this was written, against 4 GiB of address space.
 */

import { describe, expect, it } from 'vitest';

import { initFossilExecutor } from '../src/index.js';
import { countOf, runShop } from './shop.js';

const PEOPLE = 200_000;

describe('the executor at 200,000 people and 600,000 orders', () => {
  it('writes every order, inside a gigabyte of linear memory', async () => {
    const { files } = await runShop(PEOPLE);
    expect(countOf(files, 'vertex/Order/vertex_count')).toBe(BigInt(3 * PEOPLE));
    expect(countOf(files, 'vertex/Person/vertex_count')).toBeGreaterThan(0n);

    const { memory } = (await initFossilExecutor()) as { memory: WebAssembly.Memory };
    expect(memory.buffer.byteLength).toBeLessThan(2 ** 30);
  }, 300_000);
});
