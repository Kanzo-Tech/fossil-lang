/**
 * The conventions of a `fossil/1` corpus, as executable checks.
 *
 * The corpus is documented rather than typed. That was a decision and it has a price, stated here
 * where it is paid: **a guard checks what it was asked to check and nothing more.** There is no
 * compiler between the writer and the reader, so each guard below carries, in its own definition,
 * what it proves and what it cannot — and the second field is the one worth reading.
 *
 * Every check is phrased as a **count of violations**, so the expected answer is always zero and a
 * failure can say what was violated instead of what was expected. That form has one failure mode
 * and it is the classic one: an empty corpus satisfies all of them at once. The first guard is
 * therefore about non-emptiness, and it is not a formality.
 *
 * Nothing here imports a fossil crate, an `@fossil-lang/*` package, or a Parquet library. The
 * corpus is `fossil.json` read with `JSON.parse` and Parquet read with plain SQL through the
 * `duckdb` binary, which is the position a stranger is in.
 */

import { lit, query, scalar } from "./duck.mjs";
import { parquet } from "./inspect.mjs";
import { ENTRY_POINT, FORMAT } from "./manifest.mjs";

/** A guard's answer. `failures` are violations of a convention; `notes` are what it measured. */
function result(failures = [], notes = []) {
  return { failures, notes };
}

/** `count` of `what`, phrased so a zero is silent and a non-zero names the convention broken. */
function violations(count, what) {
  return Number(count) === 0 ? [] : [`${Number(count).toLocaleString("en-US")} × ${what}`];
}

/** Every declared table whose file opened — the ones a guard over rows can ask anything of. */
const readable = (tables) => tables.filter((t) => t.rows !== null);

/**
 * The DuckDB spellings each word of the manifest's type vocabulary may be read back as.
 *
 * A set per word and not one spelling, because a manifest word names a width a reader may widen
 * to: a writer that stores a `SMALLINT` and declares `int32` has told a reader something true. A
 * word missing from this table is reported and not failed — the vocabulary is the writer's, and
 * this table is only what the checker knows how to hold a column against.
 */
const SPELLINGS = {
  bool: ["BOOLEAN"],
  int8: ["TINYINT"],
  int16: ["TINYINT", "SMALLINT"],
  int32: ["TINYINT", "SMALLINT", "INTEGER"],
  int64: ["TINYINT", "SMALLINT", "INTEGER", "BIGINT"],
  uint8: ["UTINYINT"],
  uint16: ["UTINYINT", "USMALLINT"],
  uint32: ["UTINYINT", "USMALLINT", "UINTEGER"],
  uint64: ["UTINYINT", "USMALLINT", "UINTEGER", "UBIGINT"],
  float: ["FLOAT"],
  float32: ["FLOAT"],
  double: ["FLOAT", "DOUBLE"],
  float64: ["FLOAT", "DOUBLE"],
  string: ["VARCHAR"],
  date: ["DATE"],
  time: ["TIME"],
  binary: ["BLOB"],
};

/** Whether DuckDB's `actual` is a reading of the manifest word `declared`; `null` when unknown. */
function spells(declared, actual) {
  if (declared === "timestamp") return actual.startsWith("TIMESTAMP");
  if (declared.startsWith("decimal")) return actual.startsWith("DECIMAL");
  if (declared.startsWith("list<")) return actual.endsWith("[]");
  const known = SPELLINGS[declared];
  return known === undefined ? null : known.includes(actual);
}

const NUMERIC = new Set([
  "TINYINT", "SMALLINT", "INTEGER", "BIGINT", "HUGEINT",
  "UTINYINT", "USMALLINT", "UINTEGER", "UBIGINT", "FLOAT", "DOUBLE",
]);

/** The fixed columns of a table, by the fields of its manifest entry that name them. */
function fixedColumns(table) {
  const e = table.entry;
  if (table.kind === "vertex") return [e.key, e.identity].filter((c) => typeof c === "string");
  return [e.source?.key, e.destination?.key].filter((c) => typeof c === "string");
}

export const GUARDS = [
  {
    id: "not-empty",
    title: "The corpus is not empty",
    proves:
      "Every other guard is a count of violations, so an empty corpus satisfies all of them at " +
      "once. This one asserts the manifest declares at least one vertex table and one with a " +
      "`position`, and that every declared table — vertices and edges — holds rows. A table with " +
      "no rows is a relation a reader creates a view over and draws nothing from, and a writer " +
      "that has nothing to say about a type does not declare it.",
    cannotProve:
      "That the corpus is complete. A corpus missing half its vertices is non-empty, and this " +
      "guard counts what is there rather than what was promised. `declared-count` is the one that " +
      "holds each `record_count` against the disk.",
    run(corpus) {
      const failures = [];
      if (corpus.vertices.length === 0) failures.push("the manifest declares no vertex table");
      if (corpus.vertices.length > 0 && !corpus.vertices.some((t) => t.entry.position)) {
        failures.push("no vertex table declares a position, so there is nothing to draw");
      }
      for (const table of [...corpus.vertices, ...corpus.edges]) {
        if (table.rows === 0n) failures.push(`${table.kind} table ${table.name} has no rows`);
      }
      return result(
        failures,
        [...corpus.vertices, ...corpus.edges].map(
          (t) => `${t.name}: ${t.rows === null ? "?" : t.rows.toLocaleString("en-US")} rows`,
        ),
      );
    },
  },

  {
    id: "entry-point",
    title: "One entry point, and everything it names resolves",
    proves:
      "A reader that cannot list a directory — which is every reader over HTTP — starts at " +
      "`fossil.json` and reaches everything else from there. The file parses, declares `format: " +
      "\"fossil/1\"`, names every table once, every `path` it names is on disk, and every " +
      "`references` names a declared vertex table. And the converse, which is the half a reader " +
      "cannot check for itself: every Parquet file under the root is named by the manifest, so no " +
      "byte was written that a reader holding only the manifest can never ask for.",
    cannotProve:
      "That the manifest says what the files hold. Every path resolving is where this stops; the " +
      "columns are `plain-parquet`'s question and the counts `declared-count`'s.",
    run(corpus) {
      const { manifest } = corpus;
      if (manifest.error !== null) return result([manifest.error]);
      const failures = [];
      if (manifest.json.format !== FORMAT) {
        failures.push(
          `${ENTRY_POINT} declares format ${JSON.stringify(manifest.json.format)}, and a ` +
            `${FORMAT} reader refuses anything else before reading a byte of Parquet`,
        );
      }
      const tables = [...corpus.vertices, ...corpus.edges];
      const seen = new Set();
      for (const table of tables) {
        if (seen.has(table.name)) failures.push(`${table.name} is declared twice`);
        seen.add(table.name);
        if (table.path === null) failures.push(`${table.name} declares no path`);
        else if (!table.exists) failures.push(`${table.name} names ${table.path}, which is not on disk`);
        else if (table.error !== null) failures.push(`${table.path} did not open: ${table.error}`);
      }
      for (const edge of corpus.edges) {
        for (const end of ["source", "destination"]) {
          const references = edge.entry[end]?.references;
          if (corpus.vertex(references) === undefined) {
            failures.push(`${edge.name}'s ${end} references ${references}, which is no vertex table`);
          }
        }
      }
      const named = new Set(tables.map((t) => t.path));
      for (const file of corpus.onDisk) {
        if (!named.has(file)) failures.push(`${file} is on disk and ${ENTRY_POINT} does not name it`);
      }
      return result(failures, [`${tables.length} table(s) declared, ${corpus.onDisk.length} Parquet file(s) on disk`]);
    },
  },

  {
    id: "plain-parquet",
    title: "Plain Parquet, and the columns are the ones declared",
    proves:
      "Every table opens with `read_parquet` and no extension. Its columns are exactly the " +
      "`properties` its entry declares — no column a reader cannot find in the manifest, no " +
      "property missing from the bytes — and each is stored as a width its declared `type` names. " +
      "The fixed columns are held to the one width the format fixes: a vertex table's `key` and " +
      "an edge table's `source.key` and `destination.key` are `UINTEGER`, and `identity` is text.",
    cannotProve:
      "That a column carries what its name suggests. A `type` word this checker has no spelling " +
      "for is reported and not failed — the vocabulary is the writer's.",
    run(corpus) {
      const failures = [];
      const notes = [];
      for (const table of readable([...corpus.vertices, ...corpus.edges])) {
        const properties = Array.isArray(table.entry.properties) ? table.entry.properties : [];
        const declared = new Map(properties.map((p) => [p.name, p.type]));
        for (const [name, type] of declared) {
          const actual = table.columns.get(name);
          if (actual === undefined) {
            failures.push(`${table.name} declares ${name}, which its file does not carry`);
            continue;
          }
          const ok = typeof type === "string" ? spells(type, actual) : false;
          if (ok === false) failures.push(`${table.name}.${name} is declared ${type} and stored as ${actual}`);
          if (ok === null) notes.push(`${table.name}.${name}: no spelling known for ${type} (stored as ${actual})`);
        }
        for (const name of table.columns.keys()) {
          if (!declared.has(name)) failures.push(`${table.name} carries ${name}, which its entry does not declare`);
        }
        for (const name of fixedColumns(table)) {
          const actual = table.columns.get(name);
          const want = table.kind === "vertex" && name === table.entry.identity ? "VARCHAR" : "UINTEGER";
          if (actual !== undefined && actual !== want) {
            failures.push(`${table.name}.${name} is a fixed column and is ${actual}, not ${want}`);
          }
        }
        if (table.kind === "vertex" && fixedColumns(table).length < 2) {
          failures.push(`${table.name} does not name its key and its identity`);
        }
        if (table.kind === "edge" && fixedColumns(table).length < 2) {
          failures.push(`${table.name} does not name the key of both its ends`);
        }
      }
      return result(failures, notes);
    },
  },

  {
    id: "declared-count",
    title: "Each table says how many rows it holds, and they are there",
    proves:
      "`record_count` is the rows in that table's file. **This is the only guard a truncated table " +
      "fails**: a reader sizes its buffers from the manifest before it reads a byte, and a file " +
      "that lost its tail reads clean and short. Together with `dense-ids` it also fixes the " +
      "largest id in the graph at `Σ record_count − 1` over the vertex tables.",
    cannotProve:
      "That the count is the *right* count. It is written by the writer and checked against bytes " +
      "the same writer produced: both wrong together passes, and only a second writer shows it.",
    run(corpus) {
      const failures = [];
      for (const table of [...corpus.vertices, ...corpus.edges]) {
        const declared = table.entry.record_count;
        if (!Number.isSafeInteger(declared) || declared < 0) {
          failures.push(`${table.name} declares no record_count a reader can size a buffer from`);
          continue;
        }
        if (table.rows !== null && BigInt(declared) !== table.rows) {
          failures.push(`${table.name} declares ${declared} rows and its file holds ${table.rows}`);
        }
      }
      return result(failures);
    },
  },

  {
    id: "dense-ids",
    title: "`dense_id` is one gapless 0..V−1 over the whole graph, the drawn vertices first",
    proves:
      "Over the union of the vertex tables the keys are exactly `0..V−1`, with no gap, no repeat " +
      "and no null, so a reader holding one buffer per column indexes it BY the id. And every " +
      "table with a `position` holds ids below every table without one: the drawn vertices are " +
      "`0..V_placed−1`, so a buffer sized by the drawn tables' counts holds every id an edge " +
      "between drawn vertices can name.",
    cannotProve:
      "That the numbering means anything spatial. The ids are the Hilbert rank of the position " +
      "when the writer is fossil's, and that is not checked: the order is not part of the " +
      "contract, and a gapless numbering in any order passes here.",
    run(corpus) {
      const tables = readable(corpus.vertices).filter((t) => t.columns.has(t.entry.key));
      if (tables.length === 0) return result();
      const union = tables
        .map((t) => `SELECT "${t.entry.key}"::UBIGINT AS id, ${t.entry.position ? 1 : 0} AS placed FROM ${parquet(t.file)}`)
        .join(" UNION ALL ");
      const row = query(
        `SELECT min(id) AS lo, max(id) AS hi, count(*) AS n, count(DISTINCT id) AS distinct_n,
                count(*) FILTER (id IS NULL) AS nulls,
                max(id) FILTER (placed = 1) AS placed_hi, min(id) FILTER (placed = 0) AS unplaced_lo
           FROM (${union})`,
      )[0];
      const failures = [];
      if (Number(row.nulls) > 0) failures.push(`${row.nulls} vertex(es) have no dense_id`);
      if (Number(row.lo) !== 0) failures.push(`the graph starts at dense_id ${row.lo}, not 0`);
      if (Number(row.hi) !== Number(row.n) - 1) {
        failures.push(`the graph has ${row.n} vertices and a largest dense_id of ${row.hi}`);
      }
      if (Number(row.distinct_n) !== Number(row.n)) {
        failures.push(`${Number(row.n) - Number(row.distinct_n)} dense_id(s) are held by two vertices`);
      }
      if (row.placed_hi !== null && row.unplaced_lo !== null && Number(row.unplaced_lo) < Number(row.placed_hi)) {
        failures.push(
          `a vertex with no position holds dense_id ${row.unplaced_lo}, below a drawn vertex's ${row.placed_hi}`,
        );
      }
      return result(failures, [`${Number(row.n).toLocaleString("en-US")} vertices in ${tables.length} table(s)`]);
    },
  },

  {
    id: "sorted-by-key",
    title: "Every table is written in the order of its key",
    proves:
      "A vertex table's rows ascend strictly by `dense_id`, and an edge table's by `(src, dst)`, " +
      "in file order. That order is what lets DuckDB skip a row group from its footer statistics: " +
      "an id range, or an edge's source range, is a few row groups rather than all of them.",
    cannotProve:
      "That the row groups are the size the writer was asked for. The order is checked row by row " +
      "and says nothing about where the file is cut.",
    run(corpus) {
      const failures = [];
      for (const table of readable([...corpus.vertices, ...corpus.edges])) {
        const keys = fixedColumns(table).filter((c) => c !== table.entry.identity);
        if (!keys.every((k) => table.columns.has(k))) continue;
        const [a, b = a] = keys;
        const lag = (c) => `lag("${c}") OVER (ORDER BY file_row_number)`;
        const out = table.kind === "vertex" ? "a <= pa" : "a < pa OR (a = pa AND b < pb)";
        const bad = scalar(
          `SELECT count(*) FROM (
             SELECT "${a}" AS a, "${b}" AS b, ${lag(a)} AS pa, ${lag(b)} AS pb
               FROM read_parquet('${lit(table.file)}', file_row_number = true))
            WHERE pa IS NOT NULL AND (${out})`,
        );
        failures.push(...violations(bad, `${table.name}: a row out of ${keys.join(", ")} order`));
      }
      return result(failures);
    },
  },

  {
    id: "no-dangling-endpoint",
    title: "Every endpoint is a vertex of the table it references",
    proves:
      "Each edge table's `src` is a `dense_id` of its `source.references` table and its `dst` one " +
      "of its `destination.references` table. The ids are global, so an endpoint that is a vertex " +
      "of the WRONG type is in range and wrong — which is why this is a join against the named " +
      "table and not a range check.",
    cannotProve:
      "That an endpoint addresses the *right* vertex of that table. A renumbering that permuted two " +
      "ids of one type leaves every endpoint in its table; only the subjects would show it.",
    run(corpus) {
      const failures = [];
      for (const edge of readable(corpus.edges)) {
        for (const end of ["source", "destination"]) {
          const { key, references } = edge.entry[end] ?? {};
          const vertex = corpus.vertex(references);
          if (!vertex || vertex.rows === null || !edge.columns.has(key)) continue;
          const bad = scalar(
            `SELECT count(*) FROM ${parquet(edge.file)} e
              WHERE NOT EXISTS (SELECT 1 FROM ${parquet(vertex.file)} v WHERE v."${vertex.entry.key}" = e."${key}")`,
          );
          failures.push(...violations(bad, `${edge.name}: ${key} names no ${references}`));
        }
      }
      return result(failures);
    },
  },

  {
    id: "identity-is-the-subject",
    title: "The identity is the subject, not the address",
    proves:
      "The column a vertex table names as its `identity` is non-null and unique in that table: it " +
      "is the key that survives a rewrite. A re-layout renumbers every vertex, so `dense_id` is an " +
      "address and cannot also be an identity — a selection or a link from outside that stored a " +
      "`dense_id` names a different vertex after the next write.",
    cannotProve:
      "That the subject is the one the program meant. Unique and present is all this sees.",
    run(corpus) {
      const failures = [];
      for (const table of readable(corpus.vertices)) {
        const column = table.entry.identity;
        if (!table.columns.has(column)) continue;
        const row = query(
          `SELECT count(*) FILTER ("${column}" IS NULL) AS nulls,
                  count(*) - count(DISTINCT "${column}") AS repeats
             FROM ${parquet(table.file)}`,
        )[0];
        failures.push(...violations(row.nulls, `${table.name}: a vertex has no ${column}`));
        failures.push(...violations(row.repeats, `${table.name}: two vertices share a ${column}`));
      }
      return result(failures);
    },
  },

  {
    id: "declared-position",
    title: "A declared position names two numeric columns the table carries",
    proves:
      "Where a vertex table declares `position`, `by` is `layout` or `program`, and `x` and `y` " +
      "name numeric columns of its file. A `layout` position has no null: the layout places every " +
      "vertex it is given. A table with no `position` is one a reader does not draw, which is a " +
      "declaration and not a gap.",
    cannotProve:
      "That the positions are any good, or that a `program` position's nulls are intended — a " +
      "program may leave a vertex unplaced, and this reports how many it left rather than failing.",
    run(corpus) {
      const failures = [];
      const notes = [];
      for (const table of readable(corpus.vertices)) {
        const position = table.entry.position;
        if (position === undefined || position === null) {
          notes.push(`${table.name}: no position — not drawn`);
          continue;
        }
        if (position.by !== "layout" && position.by !== "program") {
          failures.push(`${table.name}: position.by is ${JSON.stringify(position.by)}, not layout or program`);
        }
        const axes = [position.x, position.y];
        const missing = axes.filter((c) => !NUMERIC.has(table.columns.get(c) ?? ""));
        if (missing.length > 0) {
          failures.push(`${table.name}: position names ${missing.join(", ")}, which is no numeric column`);
          continue;
        }
        const nulls = scalar(
          `SELECT count(*) FROM ${parquet(table.file)} WHERE "${position.x}" IS NULL OR "${position.y}" IS NULL`,
        );
        if (position.by === "layout") failures.push(...violations(nulls, `${table.name}: a laid-out vertex with no position`));
        else notes.push(`${table.name}: ${nulls} vertex(es) the program left unplaced`);
      }
      return result(failures, notes);
    },
  },
];

/** Run every guard, or the subset whose ids are given. */
export function runAll(corpus, only = null) {
  return GUARDS.filter((g) => only === null || only.includes(g.id)).map((guard) => {
    const started = Date.now();
    try {
      const { failures, notes } = guard.run(corpus);
      return { guard, failures, notes, ms: Date.now() - started };
    } catch (error) {
      return { guard, failures: [`the guard could not run: ${error.message}`], notes: [], ms: Date.now() - started };
    }
  });
}
