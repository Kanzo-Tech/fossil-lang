/**
 * The shop program at the scale that used to trap: 200,000 people and 600,000 orders.
 *
 * `DataFusion`'s default pool summed reservations into a `usize` it never checked, and the sort
 * above `Order`'s dedup charged each of its seventy-three slices for the whole batch it was cut
 * from — 5.6 GiB accounted for a 78 MiB batch. On `wasm32` that sum wrapped and a reservation
 * split unwrapped a `None`: `unreachable`, mid-run, with the promise never settled. 60,000 people
 * ran. `crates/fossil-df/src/memory.rs` is the fix and says why.
 *
 * The run must finish, every order must be written, and the executor's linear memory must stay
 * under a gigabyte — it measured 624 MiB when this was written, against 4 GiB of address space.
 */

import { initFossilExecutor } from '@fossil-lang/executor';
import { describe, expect, it } from 'vitest';

import { runShop } from './shop.js';

const PEOPLE = 200_000;

describe('the executor at 200,000 people and 600,000 orders', () => {
  it('writes every order, inside a gigabyte of linear memory', async () => {
    const { files } = await runShop(PEOPLE);
    const manifest = JSON.parse(new TextDecoder().decode(files.find((f) => f.path === 'fossil.json')!.bytes)) as {
      vertex_tables: { name: string; record_count: number }[];
    };
    const count = (name: string) => manifest.vertex_tables.find((t) => t.name === name)?.record_count;
    expect(count('Order')).toBe(3 * PEOPLE);
    expect(count('Person')).toBeGreaterThan(0);

    const { memory } = (await initFossilExecutor()) as { memory: WebAssembly.Memory };
    expect(memory.buffer.byteLength).toBeLessThan(2 ** 30);
  }, 300_000);
});
