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

import { ident, lit, query, scalar } from "./duck.mjs";
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
  double: ["FLOAT", "DOUBLE"],
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

const RR = "http://www.w3.org/ns/r2rml#";
const XSD = "http://www.w3.org/2001/XMLSchema#";

/**
 * The lexical space of each XSD datatype this checker knows, as the regular expression XSD 1.1
 * Part 2 gives it (§3.3). A datatype missing from this table is reported and not failed, as a type
 * word missing from `SPELLINGS` is: the shape's vocabulary is the shape's.
 */
const TZ = "(Z|[+-]((0[0-9]|1[0-3]):[0-5][0-9]|14:00))?";
const YEAR = "-?([1-9][0-9]{3,}|0[0-9]{3})";
const DATE = `${YEAR}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])`;
const TIME = "(([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](\\.[0-9]+)?|24:00:00(\\.0+)?)";
const INTEGER = "[+-]?[0-9]+";
const DOUBLE = "[+-]?([0-9]+(\\.[0-9]*)?|\\.[0-9]+)([Ee][+-]?[0-9]+)?|[+-]?INF|NaN";
const LEXICAL = {
  boolean: "true|false|1|0",
  decimal: "[+-]?([0-9]+(\\.[0-9]*)?|\\.[0-9]+)",
  double: DOUBLE,
  float: DOUBLE,
  date: `${DATE}${TZ}`,
  time: `${TIME}${TZ}`,
  dateTime: `${DATE}T${TIME}${TZ}`,
  gYear: `${YEAR}${TZ}`,
  hexBinary: "([0-9a-fA-F]{2})*",
  string: ".*",
  anyURI: ".*",
};
for (const local of ["integer", "long", "int", "short", "byte", "nonNegativeInteger", "positiveInteger",
  "nonPositiveInteger", "negativeInteger", "unsignedLong", "unsignedInt", "unsignedShort", "unsignedByte"]) {
  LEXICAL[local] = INTEGER;
}

/**
 * A column's natural RDF lexical form, as SQL (R2RML §10.2): its text, with the two corrections a
 * DuckDB cast needs — a timestamp's `T`, and a binary's hex.
 */
function natural(column, sql) {
  if (sql.startsWith("TIMESTAMP")) return `replace(${column}::VARCHAR, ' ', 'T')`;
  if (sql === "BLOB") return `hex(${column})`;
  return `${column}::VARCHAR`;
}

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
      "once. This one asserts the manifest declares at least one vertex table, and that every " +
      "declared table — vertices and edges — holds rows. A table with " +
      "no rows is a relation a reader creates a view over and draws nothing from, and a writer " +
      "that has nothing to say about a type does not declare it.",
    cannotProve:
      "That the corpus is complete. A corpus missing half its vertices is non-empty, and this " +
      "guard counts what is there rather than what was promised. `declared-count` is the one that " +
      "holds each `record_count` against the disk.",
    run(corpus) {
      const failures = [];
      if (corpus.vertices.length === 0) failures.push("the manifest declares no vertex table");
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
    title: "`dense_id` is one gapless 0..V−1 over the whole graph, a contiguous range per table",
    proves:
      "Each vertex table holds exactly the ids `first..first+rows−1`, with no gap, no repeat and no " +
      "null, where `first` is the rows of every vertex table before it in manifest order — so the " +
      "union is `0..V−1`, a reader holding one buffer per column indexes it BY the id, and the " +
      "counts alone say which table an id belongs to.",
    cannotProve:
      "That the order inside a table is subject order. The writer's is, and a reader may not rely " +
      "on it: `subject` is the identity, `dense_id` only an address.",
    run(corpus) {
      const failures = [];
      let first = 0n;
      for (const t of corpus.vertices) {
        const declared = BigInt(t.entry.record_count ?? 0);
        if (t.rows === null || !t.columns.has(t.entry.key)) {
          first += declared;
          continue;
        }
        const row = query(
          `SELECT min(${ident(t.entry.key)})::UBIGINT AS lo, max(${ident(t.entry.key)})::UBIGINT AS hi, count(*) AS n,
                  count(DISTINCT ${ident(t.entry.key)}) AS distinct_n, count(*) FILTER (${ident(t.entry.key)} IS NULL) AS nulls
             FROM ${parquet(t.file)}`,
        )[0];
        const n = BigInt(row.n);
        if (Number(row.nulls) > 0) failures.push(`${t.name}: ${row.nulls} vertex(es) have no dense_id`);
        if (BigInt(row.distinct_n) !== n) failures.push(`${t.name}: ${n - BigInt(row.distinct_n)} dense_id(s) repeat`);
        if (n > 0n && (BigInt(row.lo) !== first || BigInt(row.hi) !== first + n - 1n)) {
          failures.push(`${t.name} holds dense_id ${row.lo}..${row.hi}, and its range is ${first}..${first + n - 1n}`);
        }
        first += n;
      }
      return result(failures, [`${first.toLocaleString("en-US")} vertices in ${corpus.vertices.length} table(s)`]);
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
        const lag = (c) => `lag(${ident(c)}) OVER (ORDER BY file_row_number)`;
        const out = table.kind === "vertex" ? "a <= pa" : "a < pa OR (a = pa AND b < pb)";
        const bad = scalar(
          `SELECT count(*) FROM (
             SELECT ${ident(a)} AS a, ${ident(b)} AS b, ${lag(a)} AS pa, ${lag(b)} AS pb
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
              WHERE NOT EXISTS (SELECT 1 FROM ${parquet(vertex.file)} v WHERE v.${ident(vertex.entry.key)} = e.${ident(key)})`,
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
          `SELECT count(*) FILTER (${ident(column)} IS NULL) AS nulls,
                  count(*) - count(DISTINCT ${ident(column)}) AS repeats
             FROM ${parquet(table.file)}`,
        )[0];
        failures.push(...violations(row.nulls, `${table.name}: a vertex has no ${column}`));
        failures.push(...violations(row.repeats, `${table.name}: two vertices share a ${column}`));
      }
      return result(failures);
    },
  },

  {
    id: "declared-term",
    title: "Every value is a term of the kind its column declares",
    proves:
      "A column whose entry declares an RDF term holds only values that are one. Under `datatype`, " +
      "each value's natural lexical form (R2RML §10.2) is in the lexical space XSD gives that " +
      "datatype, so R2RML's datatype override (§10.3) makes a well-typed literal and never the " +
      "data error of §11. Under `term_type` `rr:IRI`, each value is an absolute IRI — it opens with " +
      "a scheme. A shape that declares `xsd:gYear` over an integer column is answered here.",
    cannotProve:
      "That a value is in the datatype's *value* space beyond its lexical form: `2023-02-30` is a " +
      "valid `xsd:date` lexically and no date, and an `xsd:byte` past 127 passes. A datatype " +
      "outside the XSD table here is reported and not failed, and an IRI is held to its scheme " +
      "only — RFC 3987's grammar is not.",
    run(corpus) {
      const failures = [];
      const notes = [];
      for (const table of readable([...corpus.vertices, ...corpus.edges])) {
        const properties = Array.isArray(table.entry.properties) ? table.entry.properties : [];
        for (const p of properties) {
          const sql = table.columns.get(p.name);
          if (sql === undefined || (p.datatype === undefined && p.term_type === undefined)) continue;
          const local = typeof p.datatype === "string" && p.datatype.startsWith(XSD) ? p.datatype.slice(XSD.length) : null;
          const pattern = p.term_type === `${RR}IRI` ? "[A-Za-z][A-Za-z0-9+.-]*:.*" : LEXICAL[local];
          if (pattern === undefined) {
            notes.push(`${table.name}.${p.name}: no lexical space known for ${p.datatype}`);
            continue;
          }
          const bad = scalar(
            `SELECT count(*) FROM ${parquet(table.file)}
              WHERE "${p.name}" IS NOT NULL AND NOT regexp_full_match(${natural(`"${p.name}"`, sql)}, '${lit(pattern)}')`,
          );
          const term = p.term_type === `${RR}IRI` ? "an IRI" : `a ${p.datatype}`;
          failures.push(...violations(bad, `${table.name}.${p.name}: a value that is not ${term}`));
        }
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
