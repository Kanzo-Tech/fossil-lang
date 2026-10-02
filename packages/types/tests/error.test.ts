import { describe, expect, it } from 'vitest';

import { CODES, FossilError, TITLES, helpUrl, isCode, isFossilError, type Problem } from '../src/index.js';

const overBudget: Problem<'run/over-budget'> = {
  code: 'run/over-budget',
  data: { consumer: 'ExternalSorter', requested: 1 << 20, reserved: 3 << 20, budget: 2 ** 31 },
  title: TITLES['run/over-budget'],
  detail: 'the run needs more memory than the budget',
  severity: 'error',
  cause: { code: 'engine/failed', data: {}, title: TITLES['engine/failed'], detail: 'the engine failed', severity: 'error', cause: { name: 'ResourcesExhausted', detail: 'no more' } },
};

describe('FossilError.from', () => {
  it('builds a real Error whose message is the detail and whose code and data are the problem’s', () => {
    const e = FossilError.from(overBudget);
    expect(e).toBeInstanceOf(Error);
    expect(e.name).toBe('FossilError');
    expect(e.message).toBe(overBudget.detail);
    expect(e.code).toBe('run/over-budget');
    expect(e.data.budget).toBe(2 ** 31);
    expect(e.problem).toBe(overBudget);
  });

  it('nests a problem cause as a FossilError and a foreign one as an Error of its own name', () => {
    const e = FossilError.from(overBudget);
    expect(isFossilError(e.cause, 'engine/failed')).toBe(true);
    const inner = (e.cause as FossilError).cause as Error;
    expect(isFossilError(inner)).toBe(false);
    expect(inner.name).toBe('ResourcesExhausted');
    expect(inner.message).toBe('no more');
  });

  it('survives a structured clone: the problem is plain data', () => {
    const e = FossilError.from(structuredClone(FossilError.from(overBudget).problem));
    expect(e.code).toBe('run/over-budget');
  });
});

describe('FossilError.of', () => {
  it('fills the title from the catalogue and keeps a foreign cause whole', () => {
    const engine = new TypeError('Parser Error');
    const e = FossilError.of('engine/failed', {}, { cause: engine, help: 'retry' });
    expect(e.title).toBe(TITLES['engine/failed']);
    expect(e.help).toBe('retry');
    expect(e.cause).toBe(engine);
    expect(e.problem.cause).toEqual({ name: 'TypeError', detail: 'Parser Error' });
  });

  it('puts a FossilError cause on the wire as its problem', () => {
    const inner = FossilError.of('corpus/unreadable', { path: 'x/fossil.json' });
    const outer = FossilError.of('document/unread', { documents: ['a'] }, { cause: inner });
    expect(outer.cause).toBe(inner);
    expect(outer.problem.cause).toBe(inner.problem);
  });
});

describe('a foreign cause that carries its own code', () => {
  class ApiError extends Error {
    override name = 'ApiError';
    constructor(
      readonly code: string,
      readonly data: unknown,
    ) {
      super('no such job');
    }
  }

  it('keeps an area/kind code and its data on the wire, and back on the rebuilt cause', () => {
    const e = FossilError.of('storage/host-refused', { scope: 'job j1' }, { cause: new ApiError('job/not-found', { job: 'j1' }) });
    expect(e.problem.cause).toEqual({ name: 'ApiError', detail: 'no such job', code: 'job/not-found', data: { job: 'j1' } });
    const rebuilt = FossilError.from(JSON.parse(JSON.stringify(e.problem)) as Problem).cause as Error & { code?: string; data?: unknown };
    expect(rebuilt.name).toBe('ApiError');
    expect(rebuilt.code).toBe('job/not-found');
    expect(rebuilt.data).toEqual({ job: 'j1' });
    expect(isFossilError(rebuilt)).toBe(false);
  });

  it('reads the code off any thrown object, an Error or not', () => {
    const e = FossilError.of('storage/host-refused', { scope: 'job j1' }, { cause: { code: 'store/refused' } });
    expect(e.problem.cause).toMatchObject({ name: 'Error', code: 'store/refused' });
  });

  it('ignores a code that is not area/kind, and data that is not a JSON object', () => {
    const refused = Object.assign(new Error('connect ECONNREFUSED'), { code: 'ECONNREFUSED', data: { port: 443 } });
    expect(FossilError.of('storage/host-refused', { scope: 's' }, { cause: refused }).problem.cause).toEqual({
      name: 'Error',
      detail: 'connect ECONNREFUSED',
    });
    const listed = FossilError.of('storage/host-refused', { scope: 's' }, { cause: new ApiError('job/not-found', [1]) });
    expect(listed.problem.cause).toEqual({ name: 'ApiError', detail: 'no such job', code: 'job/not-found' });
  });

  it('is not rebuilt as a FossilError when its code spells one of fossil’s', () => {
    const e = FossilError.of('storage/host-refused', { scope: 's' }, { cause: new ApiError('engine/failed', {}) });
    const rebuilt = FossilError.from(JSON.parse(JSON.stringify(e.problem)) as Problem).cause as Error & { code?: string };
    expect(rebuilt.name).toBe('ApiError');
    expect(rebuilt.code).toBe('engine/failed');
    expect(isFossilError(rebuilt)).toBe(false);
  });
});

describe('isFossilError', () => {
  it('is structural: an Error named FossilError with a catalogued code passes, whatever built it', () => {
    const fromWasm = Object.assign(new Error('x'), { name: 'FossilError', code: 'api/busy', data: { call: 'run' } });
    expect(isFossilError(fromWasm)).toBe(true);
    expect(isFossilError(fromWasm, 'api/busy')).toBe(true);
    expect(isFossilError(fromWasm, 'run/over-budget')).toBe(false);
  });

  it('refuses a code outside the catalogue, another name, and non-objects', () => {
    expect(isFossilError(Object.assign(new Error('x'), { name: 'FossilError', code: 'nope/nope' }))).toBe(false);
    expect(isFossilError(Object.assign(new Error('x'), { name: 'OverBudget', code: 'run/over-budget' }))).toBe(false);
    expect(isFossilError('run/over-budget')).toBe(false);
    expect(isFossilError(null)).toBe(false);
  });

  it('narrows data to the code’s fields', () => {
    const e: unknown = FossilError.from(overBudget);
    if (!isFossilError(e, 'run/over-budget')) throw new Error('expected an over-budget error');
    const { consumer, requested, reserved, budget } = e.data;
    expect([consumer, requested, reserved, budget]).toEqual(['ExternalSorter', 1 << 20, 3 << 20, 2 ** 31]);
  });
});

describe('the catalogue', () => {
  it('has a title for every code', () => {
    for (const code of CODES) expect(TITLES[code]).toBeTruthy();
  });

  it('links a code to its page', () => {
    expect(helpUrl('run/over-budget')).toBe('https://kanzo-tech.github.io/fossil-lang/docs/errors/run/over-budget');
    expect(helpUrl('api/busy', 'http://localhost:3000/')).toBe('http://localhost:3000/docs/errors/api/busy');
  });
});

describe('FossilError.of, its detail', () => {
  it('renders the detail from the data, as the Rust #[error] does', () => {
    expect(FossilError.of('storage/host-refused', { scope: 'its connections' }).message).toBe(
      'the host refused its connections',
    );
    expect(FossilError.of('corpus/duplicate-table', { table: 'Person' }).message).toBe(
      'the corpus declares Person twice',
    );
  });
});

describe('isCode', () => {
  it('holds every catalogued code, a digit after a word’s first letter, and a host’s own', () => {
    for (const code of CODES) expect(isCode(code)).toBe(true);
    expect(isCode('source/not-utf8')).toBe(true);
    expect(isCode('job/still-running')).toBe(true);
    expect(isCode('graph/no-webgl')).toBe(true);
  });

  it.each([
    ['a word that starts with a digit', '8bit/x'],
    ['an upper-case letter', 'Job/still-running'],
    ['a third word', 'a/b/c'],
    ['an empty kind', 'storage/'],
    ['a doubled hyphen', 'storage/host--silent'],
    ['a trailing hyphen', 'storage/host-'],
    ["Node's code", 'ECONNREFUSED'],
    ['a number', 404],
    ['nothing', undefined],
  ])('refuses %s', (_, value) => {
    expect(isCode(value)).toBe(false);
  });
});
