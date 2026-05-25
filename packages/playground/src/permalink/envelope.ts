/**
 * Permalink envelope — schema-versioned wrapper around playground state
 * (PLAY-04, per 09-CONTEXT.md decisions section + 09-RESEARCH.md Pattern 1).
 *
 * Why a versioned envelope: a permalink generated TODAY must still decode in
 * a future v0.2 playground (paper-permanence requirement: an ESWC/ISWC paper
 * cites the URL in 2026; reviewers click it in 2027). We achieve forward
 * compatibility by stamping every payload with `v: SCHEMA_VERSION` and
 * registering migrations from v→v+1 in the `migrations` table. The decoder
 * walks the table until it reaches the current version. Adding a v2 field is
 * therefore additive — old v1 fixtures keep deciding via the migration path
 * tested in CI today (see test/permalink-forward-compat.test.ts).
 *
 * Why pure functions, no DOM globals: this module is tested in Node Vitest
 * (no browser shim). `btoa`/`atob` are Node 16+ globals; `fflate` is
 * isomorphic. Keeping the envelope side-effect-free means it can also be
 * called from `apps/landing` SSR or a future CLI sub-command without porting.
 *
 * Stack: `fflate` (NOT pako) per 09-RESEARCH.md §Stack — ESM-first, ~12 KB
 * min+gz, zero deps. CompressionStream was considered (Baseline-2024) but a
 * single library path simplifies Node-CI parity.
 */

export const SCHEMA_VERSION = 1 as const;

/**
 * URL-fragment safety budget. Chosen as 8000 bytes because:
 *   - Edge legacy IE inheritance caps URLs at ~2,083 chars total (path + query
 *     + fragment). Modern Edge raised this but we keep a conservative floor.
 *   - Safari practical display truncates ~10 KB; we leave ~2 KB headroom for
 *     origin + path.
 *   - 8 KB is what TypeScript Playground uses as a soft warn boundary; we
 *     adopt the same threshold for cross-tool familiarity.
 * The encoder throws PermalinkTooLargeError above this; the UI (PLAY-04 in
 * plan 09-05) is expected to render a "share via Gist" affordance when the
 * encoded length nears the cap.
 */
export const MAX_PERMALINK_BYTES = 8_000;

export type PermalinkStateV1 = {
  v: 1;
  /** .fossil source text — the mapping the user is editing. */
  source: string;
  /** Optional CSVW JSON-LD descriptor (PLAY-09 + PLAY-11 wire this). */
  csvw?: string;
  /** Optional ShEx target shape text. */
  shex?: string;
};

/**
 * Migration registry. Each key migrates from v→v+1.
 *
 * Currently SCHEMA_VERSION=1 so the table is empty. The mere existence of
 * the registry — and the forward-compat test that exercises it via committed
 * v1 fixtures — IS the future-proofing mechanism. When v0.2 ships:
 *
 *     migrations[1] = (s) => ({ ...s, v: 2, newField: defaultValue });
 *
 * The test `permalink-forward-compat.test.ts` will then decode the 2026
 * fixtures via the new migration and assert the v2 shape — proving the
 * upgrade is backward compatible.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const migrations: Record<number, (s: any) => any> = {
  // 1: (s) => ({ ...s, v: 2, newField: defaultValue }),   // FUTURE
};

export function migrate(
  state: { v: number } & Record<string, unknown>,
): PermalinkStateV1 {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let cur: any = state;
  while (cur.v < SCHEMA_VERSION) {
    const m = migrations[cur.v];
    if (!m) throw new Error(`No migration from v${cur.v}`);
    cur = m(cur);
  }
  if (cur.v !== SCHEMA_VERSION) {
    throw new Error(
      `Permalink schema version ${cur.v} is newer than supported ${SCHEMA_VERSION}; upgrade the playground`,
    );
  }
  return cur as PermalinkStateV1;
}

/**
 * base64url encoding (RFC 4648 §5) — URL-safe, no padding.
 *
 * We intentionally use the classic `btoa(String.fromCharCode(...u8))` path
 * because it has zero allocation overhead vs the fancier APIs and works
 * identically in Node 16+ and every evergreen browser. A loop-based bin
 * construction avoids the call-stack limit that would hit for >100 KB inputs
 * via the spread operator `String.fromCharCode(...u8)`.
 */
export function toB64Url(u8: Uint8Array): string {
  let bin = '';
  for (const b of u8) bin += String.fromCharCode(b);
  return btoa(bin)
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replaceAll('=', '');
}

export function fromB64Url(s: string): Uint8Array {
  const b64 =
    s.replaceAll('-', '+').replaceAll('_', '/') +
    '==='.slice((s.length + 3) % 4);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
