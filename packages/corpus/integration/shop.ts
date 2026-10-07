import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

import { initFossilExecutor, run, type CorpusFile } from '@fossil-lang/executor';
import type { RunReport } from '@fossil-lang/types';

const PROGRAM = new URL('../../../docs/programs/shop/', import.meta.url);

/** The program's two CSVs at `people` users and three orders each, from a fixed LCG so a failure reproduces. */
function sources(people: number): { users: string; orders: string } {
  let state = 7;
  const next = () => (state = (state * 1_103_515_245 + 12_345) >>> 0) / 2 ** 32;
  const users = ['id,email,name,age'];
  for (let i = 0; i < people; i += 1) users.push(`${i},u${i}@shop.example,User ${i},${10 + Math.floor(next() * 70)}`);
  const orders = ['id,user_id,amount'];
  for (let i = 0; i < 3 * people; i += 1) orders.push(`${i},${Math.floor(next() * people)},${(next() * 500).toFixed(2)}`);
  return { users: `${users.join('\n')}\n`, orders: `${orders.join('\n')}\n` };
}

/**
 * `docs/programs/shop/shop.fossil` run through `@fossil-lang/executor` — the only host that writes a
 * corpus — in this process, over `people` generated users. Answers what the run wrote.
 */
export async function runShop(people: number): Promise<{ report: RunReport; files: CorpusFile[] }> {
  const wasm = createRequire(import.meta.url).resolve('@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm');
  await initFossilExecutor(readFileSync(wasm));
  const base = 'https://local.test/shop/';
  const { users, orders } = sources(people);
  const encode = (text: string) => new TextEncoder().encode(text);
  return run(readFileSync(new URL('shop.fossil', PROGRAM), 'utf8'), {
    files: {
      [`${base}shop.shex`]: readFileSync(new URL('shop.shex', PROGRAM)),
      [`${base}data/users.csv`]: encode(users),
      [`${base}data/orders.csv`]: encode(orders),
    },
    path: `${base}shop.fossil`,
  });
}
