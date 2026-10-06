/**
 * A conforming `fossil/1` corpus, written by something that is not fossil.
 *
 * This is the checker's own evidence. A guard suite with no corpus to run on is a set of sentences;
 * one that only ever runs on the corpus its authors wrote is a set of sentences about themselves.
 * So the fixture is written here, in JavaScript and SQL, against the published conventions and
 * nothing else — no Rust, no `@fossil-lang/*`. It is also what `self-test.mjs` mutates: every
 * guard is proved to fire by breaking exactly one convention in a corpus that satisfies all of them.
 *
 * The graph is one graph, as the format is: `Person`, `Order` and `Tag` share one `dense_id` space,
 * each type one contiguous range of it in manifest order, subject order inside it. Three
 * relations, one inside a type and two between types, and one multi-valued property: a person's
 * nicknames, a row each.
 *
 * **What it is not.** It is not a benchmark and not a realistic graph. Nothing here is normative.
 * The conventions are.
 *
 *   node guards/fixture.mjs <dir> [--vertices 70000] [--orders N] [--tags 16]
 */

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { execute, lit } from "./duck.mjs";
import { ENTRY_POINT, FORMAT } from "./manifest.mjs";

/** Rows per row group — DuckDB's own default, stated rather than inherited. */
export const ROW_GROUP_ROWS = 122_880;

const BASE = "https://example.org/";

/**
 * Each of `subjects`' ids: `first` plus its rank in subject order — the writer's numbering, where a
 * type is one contiguous range and its rows are sorted by `subject`.
 */
function numbered(subjects, first) {
  const ids = new Uint32Array(subjects.length);
  subjects
    .map((s, k) => ({ s, k }))
    .sort((a, b) => (a.s < b.s ? -1 : a.s > b.s ? 1 : 0))
    .forEach(({ k }, rank) => (ids[k] = first + rank));
  return ids;
}

/** Write `rows` as CSV, a line at a time, so a million rows is not one string. */
function csv(path, header, rows) {
  const lines = [header];
  for (const row of rows) lines.push(row.map((v) => (v === null ? "" : String(v))).join(","));
  writeFileSync(path, `${lines.join("\n")}\n`);
}

/**
 * Write the corpus to `dir`, replacing whatever is there.
 *
 * `count` people, each knowing the next one round a ring and one other; `orders` orders, each placed
 * by one person; `tags` tags, a third of the people tagged with one.
 */
export function write(dir, { count = 70_000, orders, tags = 16 } = {}) {
  orders ??= Math.ceil(count / 4);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, "vertex"), { recursive: true });
  mkdirSync(join(dir, "edge"), { recursive: true });
  mkdirSync(join(dir, "property"), { recursive: true });

  const placedBy = (j) => (j * 7919) % count;
  const personSubjects = Array.from({ length: count }, (_, i) => `${BASE}person/${i}`);
  const orderSubjects = Array.from({ length: orders }, (_, j) => `${BASE}order/${j}`);
  const tagSubjects = Array.from({ length: tags }, (_, k) => `${BASE}tag/${k}`);
  const personId = numbered(personSubjects, 0);
  const orderId = numbered(orderSubjects, count);
  const tagId = numbered(tagSubjects, count + orders);

  const knows = new Set();
  const pairs = [];
  const link = (a, b) => {
    const key = `${a},${b}`;
    if (a === b || knows.has(key)) return;
    knows.add(key);
    pairs.push([a, b]);
  };
  for (let i = 0; i < count; i += 1) {
    link(i, (i + 1) % count);
    link(i, (i * 7 + 3) % count);
  }

  const scratch = join(dir, ".csv");
  mkdirSync(scratch, { recursive: true });
  const at = (name) => join(scratch, `${name}.csv`);
  csv(at("Person"), "dense_id,subject,birth_year,postcode",
    personSubjects.map((s, i) => [personId[i], s, i % 13 === 0 ? null : 1940 + (i % 70), `PC${String(i % 997).padStart(4, "0")}`]));
  csv(at("Order"), "dense_id,subject,amount",
    orderSubjects.map((s, j) => [orderId[j], s, ((j * 37) % 1000) / 4]));
  csv(at("Tag"), "dense_id,subject,name", tagSubjects.map((s, k) => [tagId[k], s, `tag-${k}`]));
  csv(at("knows"), "src,dst,since", pairs.map(([a, b]) => [personId[a], personId[b], 2000 + ((a + b) % 25)]));
  csv(at("placedBy"), "src,dst", Array.from({ length: orders }, (_, j) => [orderId[j], personId[placedBy(j)]]));
  const tagged = [];
  for (let i = 0; i < count; i += 3) if (tags > 0) tagged.push([personId[i], tagId[i % tags]]);
  csv(at("tagged"), "src,dst", tagged);
  // None for one person in five, one for the rest, and a second for every other one.
  const nicknames = [];
  for (let i = 0; i < count; i += 1) {
    if (i % 5 === 0) continue;
    nicknames.push([personId[i], `nick-${i % 7}`]);
    if (i % 2 === 0) nicknames.push([personId[i], `alias-${i % 11}`]);
  }
  csv(at("nickname"), "src,nickname", nicknames);

  const P = (name, type, extra = {}) => ({ name, type, ...extra });
  // The term a shape would declare: a datatype the column's type does not imply, as a shape says it.
  const literal = (local) => ({ term_type: "http://www.w3.org/ns/r2rml#Literal", datatype: `http://www.w3.org/2001/XMLSchema#${local}` });
  const writer = [P("dense_id", "uint32", { role: "address" }), P("subject", "string", { role: "identity" })];
  const head = "dense_id::UINTEGER AS dense_id, subject::VARCHAR AS subject";
  const vertexTables = [
    {
      name: "Person", iri: `${BASE}Person`, rows: count, sql: `${head}, birth_year::INTEGER AS birth_year, postcode::VARCHAR AS postcode`,
      properties: [...writer, P("birth_year", "int32", { iri: `${BASE}birthYear`, ...literal("gYear"), nullable: true }), P("postcode", "string", { iri: `${BASE}postcode` })],
    },
    {
      name: "Order", iri: `${BASE}Order`, rows: orders, sql: `${head}, amount::DOUBLE AS amount`,
      properties: [...writer, P("amount", "double", { iri: `${BASE}amount`, ...literal("decimal") })],
    },
    {
      name: "Tag", iri: `${BASE}Tag`, rows: tags, sql: `${head}, name::VARCHAR AS name`,
      properties: [...writer, P("name", "string", { iri: `${BASE}name` })],
    },
  ].filter((t) => t.rows > 0);
  const ends = [P("src", "uint32", { role: "endpoint" }), P("dst", "uint32", { role: "endpoint" })];
  const edgeTables = [
    { src: "Person", label: "knows", dst: "Person", csv: "knows", rows: pairs.length, sql: "src::UINTEGER AS src, dst::UINTEGER AS dst, since::INTEGER AS since", properties: [...ends, P("since", "int32")] },
    { src: "Order", label: "placedBy", dst: "Person", csv: "placedBy", rows: orders, sql: "src::UINTEGER AS src, dst::UINTEGER AS dst", properties: ends },
    { src: "Person", label: "tagged", dst: "Tag", csv: "tagged", rows: tagged.length, sql: "src::UINTEGER AS src, dst::UINTEGER AS dst", properties: ends },
  ].filter((e) => e.rows > 0);
  const propertyTables = [
    {
      type: "Person", name: "nickname", rows: nicknames.length, sql: "src::UINTEGER AS src, nickname::VARCHAR AS nickname",
      properties: [P("src", "uint32", { role: "endpoint" }), P("nickname", "string", { iri: `${BASE}nickname`, ...literal("string") })],
    },
  ].filter((t) => t.rows > 0);

  const copy = (from, sql, order, to) =>
    `COPY (SELECT ${sql} FROM read_csv('${lit(from)}', header = true, all_varchar = true) ORDER BY ${order})
       TO '${lit(to)}' (FORMAT PARQUET, ROW_GROUP_SIZE ${ROW_GROUP_ROWS});`;
  execute(
    [
      // One thread, so the file is cut at the same rows on every run and the recipe reproduces.
      "SET threads = 1;",
      ...vertexTables.map((t) => copy(at(t.name), t.sql, "dense_id", join(dir, "vertex", `${t.name}.parquet`))),
      ...edgeTables.map((e) =>
        copy(at(e.csv), e.sql, "src, dst", join(dir, "edge", `${e.src}_${e.label}_${e.dst}.parquet`)),
      ),
      ...propertyTables.map((t) =>
        copy(at(t.name), t.sql, `src, ${t.name}`, join(dir, "property", `${t.type}_${t.name}.parquet`)),
      ),
    ].join("\n"),
  );
  rmSync(scratch, { recursive: true, force: true });

  // Last, because it is the commit: a reader that finds it finds every file it names.
  const manifest = {
    format: FORMAT,
    vertex_tables: vertexTables.map((t) => ({
      name: t.name,
      iri: t.iri,
      path: `vertex/${t.name}.parquet`,
      key: "dense_id",
      identity: "subject",
      record_count: t.rows,
      properties: t.properties,
    })),
    edge_tables: edgeTables.map((e) => ({
      name: `${e.src}_${e.label}_${e.dst}`,
      label: e.label,
      iri: `${BASE}${e.label}`,
      path: `edge/${e.src}_${e.label}_${e.dst}.parquet`,
      source: { key: "src", references: e.src },
      destination: { key: "dst", references: e.dst },
      record_count: e.rows,
      properties: e.properties,
    })),
    property_tables: propertyTables.map((t) => ({
      name: `${t.type}_${t.name}`,
      path: `property/${t.type}_${t.name}.parquet`,
      source: { key: "src", references: t.type },
      record_count: t.rows,
      properties: t.properties,
    })),
  };
  writeFileSync(join(dir, ENTRY_POINT), `${JSON.stringify(manifest, null, 2)}\n`);

  return {
    dir,
    vertices: vertexTables.reduce((n, t) => n + t.rows, 0),
    edges: edgeTables.reduce((n, e) => n + e.rows, 0),
    tables: vertexTables.length + edgeTables.length + propertyTables.length,
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [dir] = process.argv.slice(2).filter((a, i, all) => !a.startsWith("--") && !all[i - 1]?.startsWith("--"));
  if (!dir) {
    console.error("usage: node guards/fixture.mjs <dir> [--vertices N] [--orders N] [--tags N]");
    process.exit(2);
  }
  const flag = (name) => {
    const at = process.argv.indexOf(`--${name}`);
    return at === -1 ? undefined : Number(process.argv[at + 1]);
  };
  const written = write(dir, {
    count: flag("vertices") ?? 70_000,
    orders: flag("orders"),
    tags: flag("tags") ?? 16,
  });
  console.log(
    `${written.vertices} vertices · ${written.edges} edges · ` +
      `${written.tables} tables → ${written.dir}`,
  );
}
