/**
 * The declared bound, and the arithmetic that checks it — in the tab, over the released bytes.
 *
 * A corpus carries a `privacy:` block in `graph.graph.yml` stating what its bytes guarantee.
 * The format's claim about that block is not that it is trustworthy; it is that **every number
 * in it is re-derivable from the files** by a recipient holding nothing else. This module is
 * that recipient. It reads the declaration out of the manifest the streaming panel already
 * fetched, recomputes the same numbers with the DuckDB this app already booted, and hands both
 * to the panel to display side by side.
 *
 * ## Why re-derive rather than display
 *
 * A badge reading `k: 5 ✓` is worth very little. It restates the producer's own claim in a
 * larger font, and the producer is the party the recipient cannot check. Recomputing is a
 * different act: it is the guard `apps/corpus/guards/check.mjs` runs natively —
 * `declared-privacy` — performed by the reader, on the reader's machine, against the bytes the
 * reader actually holds. What that demonstrates is a property of the FORMAT, not a property of
 * this corpus.
 *
 * ## What the arithmetic has to get right, and what a `GROUP BY` gets wrong
 *
 * Two things, both of which a naive `GROUP BY qi HAVING count(*) < k` gets wrong by accident:
 *
 * 1. **The class is the release, not the tile.** A corpus is tiles, so a per-tile aggregation
 *    is the easiest wrong answer available: every class it finds is a subset of a real one, so
 *    it reports a `k` that is too small and looks conservative while never having been checked.
 *    The query below reads every tile of the payload as ONE relation.
 * 2. **Nulls are a declared parameter, not a convention.** SQL groups every `NULL` together;
 *    disclosure control treats an absent quasi-identifier as a wildcard matching every value.
 *    Both readings are defensible and they disagree on the same file, so the manifest declares
 *    which one it means in `absent_quasi_identifier` and this module honours it.
 *
 * See `/docs/format/conventions/privacy` for the normative statement of both.
 */
import * as duck from './duckdb.js';

/** How an absent quasi-identifier is read. The manifest declares one; a reader obeys it. */
export type AbsentReading = 'value' | 'wildcard' | 'suppress';

/**
 * The block as the manifest states it.
 *
 * Flat scalars, because two of the four readers of `graph.graph.yml` are line scanners that
 * skip what they cannot see — which is why `quasi_identifiers` is one space-separated string
 * rather than a YAML sequence.
 */
export interface DeclaredBound {
  bound: string;
  k: number;
  reached: number;
  population: number;
  suppressed: number;
  suppressionBudgetPpm: number;
  absentQuasiIdentifier: AbsentReading;
  /** `<Type>.<column>` pairs. **The one field a reader cannot derive**, and the one it all turns on. */
  quasiIdentifiers: string[];
  /** What the writer did to reach `k`, per column, or `none`. Absent in corpora written before it existed. */
  generalization: string;
  policy: string;
  profile: string;
}

/** The three states a manifest can be in, and none of them means "public". */
export type Declaration =
  | { state: 'absent' }
  | { state: 'undeclared' }
  | { state: 'declared'; bound: DeclaredBound };

/**
 * Read the `privacy:` block out of a manifest.
 *
 * A line scanner, deliberately, and the same shape `packages/corpus/src/manifest.ts` and
 * `apps/corpus/guards/manifest.mjs` use: two spaces of indent, `key: scalar`, no sequences.
 * The crate that writes it asserts that shape precisely so four independent parsers agree.
 */
export function readDeclaration(manifest: string): Declaration {
  const lines = manifest.split('\n');
  const at = lines.findIndex((l) => /^privacy:\s*$/.test(l));
  if (at < 0) return { state: 'absent' };

  const fields: Record<string, string> = {};
  for (const line of lines.slice(at + 1)) {
    const m = /^ {2}([a-z_]+):\s*(.*)$/.exec(line);
    if (!m) break;
    fields[m[1]!] = m[2]!.trim();
  }

  if (fields.bound !== 'k-anonymity') return { state: 'undeclared' };

  const num = (key: string) => Number(fields[key] ?? 0);
  return {
    state: 'declared',
    bound: {
      bound: fields.bound,
      k: num('k'),
      reached: num('reached'),
      population: num('population'),
      suppressed: num('suppressed'),
      suppressionBudgetPpm: num('suppression_budget_ppm'),
      absentQuasiIdentifier: (fields.absent_quasi_identifier ?? 'value') as AbsentReading,
      quasiIdentifiers: (fields.quasi_identifiers ?? '').split(/\s+/).filter(Boolean),
      // Absent means `none`: the quasi-identifiers were published as the program produced them,
      // which is what every corpus written before the writer could derive anything says.
      generalization: fields.generalization || 'none',
      policy: fields.policy ?? '',
      profile: fields.profile ?? '',
    },
  };
}

/** What the reader computed for itself, and what it cost. */
export interface Recomputed {
  reached: number;
  population: number;
  classes: number;
  /** Records whose quasi-identifier tuple is incomplete — what the null reading is ABOUT. */
  incomplete: number;
  ms: number;
  /** The SQL, so the panel can show the reader exactly what was asked. */
  sql: string;
}

/**
 * Recompute the bound over one vertex type's whole payload.
 *
 * `name` is the tile payload as the caller registered it with DuckDB — under
 * `container: rowgroups` that is one file holding every tile, which is what makes "the class is
 * the release" a single scan rather than a union the caller has to assemble correctly.
 *
 * The null reading changes the arithmetic and not the bytes. `value` is the conservative end
 * and the only one this corpus needs, but the other two are spelled out because a reader that
 * silently assumed one would be computing a different property from the one declared:
 *
 * - `value`     — a record with an absent quasi-identifier is a category of its own.
 * - `wildcard`  — it joins every class it could belong to, so its class is at least as large.
 * - `suppress`  — it is not certified at all, and is charged against the budget.
 */
export async function recompute(name: string, bound: DeclaredBound): Promise<Recomputed> {
  // `Person.birth_year` names a column of the Person payload; the type half is the file we
  // are already reading, so only the column half survives into the projection.
  const columns = bound.quasiIdentifiers.map((q) => q.split('.').slice(1).join('.'));
  if (columns.length === 0) throw new Error('the manifest declares no quasi-identifiers');

  const tuple = columns.map((c) => `"${c}"`).join(', ');
  const anyNull = columns.map((c) => `"${c}" IS NULL`).join(' OR ');

  // Under `suppress` the uncertified records leave before the classes are formed; under the
  // other two readings every record is counted, and `value` is what makes two NULLs equal —
  // which is SQL's own grouping, so it needs no clause of its own.
  const certified = bound.absentQuasiIdentifier === 'suppress' ? `WHERE NOT (${anyNull})` : '';

  const sql = `WITH release AS (
  SELECT ${tuple} FROM "${name}" ${certified}
), classes AS (
  SELECT count(*) AS n FROM release GROUP BY ${tuple}
)
SELECT min(n) AS reached, sum(n) AS population, count(*) AS classes FROM classes`;

  const started = performance.now();
  const [row] = await duck.query(sql);
  const [gaps] = await duck.query(`SELECT count(*) AS n FROM "${name}" WHERE ${anyNull}`);
  const ms = performance.now() - started;

  return {
    reached: Number(row?.reached ?? 0),
    population: Number(row?.population ?? 0),
    classes: Number(row?.classes ?? 0),
    incomplete: Number(gaps?.n ?? 0),
    ms,
    sql,
  };
}

/** Whether the declaration and the recomputation agree, field by field. */
export interface Agreement {
  reached: boolean;
  population: boolean;
  /** The claim the bound actually makes. Everything else is arithmetic supporting it. */
  satisfied: boolean;
  budgetHonoured: boolean;
  all: boolean;
}

export function agrees(bound: DeclaredBound, found: Recomputed): Agreement {
  const reached = bound.reached === found.reached;
  const population = bound.population === found.population;
  // Integer ppm, never a float: a manifest is a text document four independent parsers read,
  // and a float is the one scalar they can disagree about over the same bytes.
  const budgetHonoured = bound.suppressed * 1_000_000 <= bound.population * bound.suppressionBudgetPpm;
  const satisfied = found.reached >= bound.k;
  return { reached, population, satisfied, budgetHonoured, all: reached && population && satisfied && budgetHonoured };
}

/**
 * How much room the release has over what its policy asked for.
 *
 * Published because it is the number that stops a green tick from meaning more than it does:
 * a corpus reaching 5,000 against a `k` of 5 cleared its bound by three orders of magnitude,
 * which says the bound was never the binding constraint on this data — not that the data was
 * carefully protected. A reader that sees only "satisfied" learns the opposite of that.
 */
export const margin = (bound: DeclaredBound, found: Recomputed): number =>
  bound.k === 0 ? 0 : found.reached / bound.k;

/* -------------------------------------------------------------------------------------------
 * The ladder — what `reached` costs, which is the number the manifest cannot publish
 * ---------------------------------------------------------------------------------------- */

/**
 * One rung of a declared generalisation ladder.
 *
 * A rung is a whole release: every quasi-identifier rendered at one declared hierarchy level,
 * for every row. That is **global** recoding, and it is the case in which a level is a
 * comparable thing at all — `ColumnLevels::Levels` in `crates/fossil-kanon/src/report.rs` says
 * why, and `ColumnLevels::Spans` is the case where it is not.
 */
export interface Rung {
  /**
   * What a manifest written at this rung would carry in `generalization:`, in the grammar
   * `fossil_sinks::manifest` documents: `none`, or a space-separated
   * `<Type>.<column>@<coarsest>-<finest>/<declared>` per generalised column. Coarsest equals
   * finest here because a rung is one level for the whole column.
   */
  token: string;
  /** What each quasi-identifier still says at this rung, in English. */
  says: string[];
  /**
   * The whole release in one phrase, for prose.
   *
   * Separate from `says` because a sentence and a table column want different English: a cell
   * reading `1 nothing` beside a count is fine, and «publishes 1 nothing, 1 nothing» is not a
   * sentence.
   */
  release: string;
  /** The SQL rendering each quasi-identifier at this rung, in the ladder's column order. */
  exprs: string[];
}

/** A ladder over one corpus's quasi-identifiers, finest rung first. */
export interface Ladder {
  columns: string[];
  rungs: Rung[];
}

/**
 * The ladder for the bench corpus, and **where it comes from is the point**.
 *
 * It is not in the corpus. `generalization:` is checkable by a recipient who holds the
 * hierarchy, and the hierarchy lives in the policy document that `policy:` NAMES rather than
 * locates — a corpus carrying its own policy would be a corpus that can be handed on with the
 * policy rewritten. So this constant is the playground standing in for that document, and the
 * panel says so rather than letting it look derived.
 *
 * The shapes are the two `crates/fossil-kanon/hierarchies/` ships: a numeric column published
 * as its enclosing declared bucket, and a prefix column cut on characters. Prefix values keep
 * their `*` at every generalised level, including the finest, because `PC1*` and a postcode
 * that is literally `PC1` are different facts — `Prefix::value_at` makes the same choice for
 * the same reason.
 */
export const BENCH_LADDER: Ladder = {
  columns: ['birth_year', 'postcode'],
  rungs: [
    {
      token: 'none',
      says: ['the year', 'the postcode'],
      release: 'the year and the postcode, as the program produced them',
      exprs: ['"birth_year"::VARCHAR', '"postcode"'],
    },
    {
      token: 'Person.postcode@2-2/3',
      says: ['the year', 'the postcode, 3 characters'],
      release: 'the year, and 3 characters of the postcode',
      exprs: ['"birth_year"::VARCHAR', `substr("postcode", 1, 3) || '*'`],
    },
    {
      token: 'Person.birth_year@bucket Person.postcode@2-2/3',
      says: ['the decade', 'the postcode, 3 characters'],
      release: 'the decade, and 3 characters of the postcode',
      exprs: [`((("birth_year" / 10)::INTEGER) * 10)::VARCHAR || 's'`, `substr("postcode", 1, 3) || '*'`],
    },
    {
      token: 'Person.birth_year@bucket Person.postcode@1-1/3',
      says: ['the decade', 'the postcode, 2 characters'],
      release: 'the decade, and a postcode column that is PC* for every row',
      exprs: [`((("birth_year" / 10)::INTEGER) * 10)::VARCHAR || 's'`, `substr("postcode", 1, 2) || '*'`],
    },
    {
      token: 'Person.birth_year@0-0/2 Person.postcode@0-0/3',
      says: ['nothing', 'nothing'],
      release: 'two columns of * — nothing at all',
      exprs: [`'*'`, `'*'`],
    },
  ],
};

/** A rung, climbed: the release it would have produced, measured. */
export interface Climbed extends Rung {
  reached: number;
  classes: number;
  /** Distinct published values left in each column — what the release still says, as a count. */
  distinct: number[];
}

/**
 * Climb the ladder over the real bytes, and report what each rung would have released.
 *
 * This is the half a badge cannot have. Every rung satisfies k-anonymity, every rung writes a
 * `privacy:` block, and `reached` is weakly monotone up the ladder — generalising can only
 * merge classes. Weakly is the interesting word: over the bench corpus the first THREE rungs
 * all reach 5,000, publishing 40×25, 40×10 and 5×10 distinct values. Three manifests, the same
 * `reached` over the same population, three releases that are not the same release, and no
 * recomputation from the Parquet can tell them apart. That is the argument for the
 * `generalization` field stated as a measurement rather than as a worry.
 *
 * The top rung is the reductio: one class holding everybody, which satisfies **any** k the
 * population is large enough for, and publishes two columns of `*`.
 *
 * Sequential rather than parallel on purpose: five scans of the same million rows through one
 * DuckDB connection, so the elapsed time reported is a time a reader could have measured.
 */
export async function climb(name: string, ladder: Ladder): Promise<Climbed[]> {
  const out: Climbed[] = [];
  for (const rung of ladder.rungs) {
    const projected = rung.exprs.map((e, i) => `${e} AS q${i}`).join(', ');
    const tuple = rung.exprs.map((_, i) => `q${i}`).join(', ');
    const rows = await duck.query(`WITH g AS (
  SELECT ${projected} FROM "${name}"
), c AS (
  SELECT count(*) AS n FROM g GROUP BY ${tuple}
)
SELECT (SELECT count(*) FROM c) AS classes, (SELECT min(n) FROM c) AS reached,
       ${rung.exprs.map((_, i) => `(SELECT count(DISTINCT q${i}) FROM g) AS d${i}`).join(', ')}`);
    const row = rows[0] ?? {};
    out.push({
      ...rung,
      classes: Number(row.classes ?? 0),
      reached: Number(row.reached ?? 0),
      distinct: rung.exprs.map((_, i) => Number(row[`d${i}`] ?? 0)),
    });
  }
  return out;
}

/**
 * The rung a writer asked for `k` would have stopped at — the lowest one that clears it.
 *
 * `null` when no rung does, which for a ladder whose top is one class means only that `k`
 * exceeds the population. **That is the honest shape of a refusal and it is not this ladder's
 * to give**: a refusal comes from a hierarchy running out before `k` does, and a ladder ending
 * in `*` never runs out. Which is the whole argument — see the panel.
 */
export const stopsAt = (climbed: Climbed[], k: number): Climbed | null =>
  climbed.find((rung) => rung.reached >= k) ?? null;
