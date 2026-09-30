/**
 * A conforming `fossil/1` corpus, written by something that is not fossil.
 *
 * This is the checker's own evidence. A guard suite with no corpus to run on is a set of sentences;
 * one that only ever runs on the corpus its authors wrote is a set of sentences about themselves.
 * So the fixture is written here, in JavaScript and SQL, against the published conventions and
 * nothing else — no Rust, no `@fossil-lang/*`. It is also what `self-test.mjs` mutates: every
 * guard is proved to fire by breaking exactly one convention in a corpus that satisfies all of them.
 *
 * The graph is one graph, as the format is: `Person` and `Order` are drawn and share one `dense_id`
 * space, numbered by the Hilbert rank of their position over the union of the two; `Tag` has no
 * position and takes the ids after them. Three relations, one of them between two drawn types and
 * one from a drawn type to the undrawn one.
 *
 * **What it is not.** It is not a benchmark and not a realistic graph — the positions come from a
 * grid of phyllotactic discs because that is a shape with real clustering and no dependencies.
 * Nothing here is normative. The conventions are.
 *
 *   node guards/fixture.mjs <dir> [--vertices 70000] [--clusters 256] [--orders N] [--tags 16]
 */

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { execute, lit } from "./duck.mjs";
import { ENTRY_POINT, FORMAT } from "./manifest.mjs";

/** Rows per row group — DuckDB's own default, stated rather than inherited. */
export const ROW_GROUP_ROWS = 122_880;

const GOLDEN_ANGLE = 2.3999632;
const CLUSTER_SPACING = 100;
const INTRA_CLUSTER_RADIUS = 12;
const BASE = "https://example.org/";

/** Positions: `clusters` discs on a square grid, phyllotaxis-packed inside each, in binary32. */
function positions(count, clusters) {
  const side = Math.ceil(Math.sqrt(clusters));
  const per = Math.ceil(count / clusters);
  const out = new Array(count);
  for (let i = 0; i < count; i += 1) {
    const cluster = Math.floor(i / per);
    const k = i % per;
    const radius = INTRA_CLUSTER_RADIUS * Math.sqrt(k);
    const angle = GOLDEN_ANGLE * k;
    out[i] = {
      cluster,
      x: Math.fround((cluster % side) * CLUSTER_SPACING + radius * Math.cos(angle)),
      y: Math.fround(Math.floor(cluster / side) * CLUSTER_SPACING + radius * Math.sin(angle)),
    };
  }
  return { points: out, per };
}

/** `xy2d` on the order-16 Hilbert curve — the curve DuckDB's `ST_Hilbert` walks. */
function hilbert2(x, y) {
  let d = 0;
  for (let s = 1 << 15; s > 0; s >>= 1) {
    const rx = (x & s) !== 0 ? 1 : 0;
    const ry = (y & s) !== 0 ? 1 : 0;
    d += s * s * ((3 * rx) ^ ry);
    if (ry === 0) {
      if (rx === 1) {
        x ^= 0xffff;
        y ^= 0xffff;
      }
      [x, y] = [y, x];
    }
  }
  return d;
}

/** One coordinate on `0..65535` over `[lo, hi]`, by `floor` — `ST_Hilbert`'s quantisation. */
function quantize(v, lo, hi) {
  if (hi <= lo) return 0;
  return Math.min(Math.max(Math.floor(((v - lo) / (hi - lo)) * 65535), 0), 65535);
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
 * `count` people in `clusters` discs, each knowing the next one round a ring and one other person
 * of their own disc; `orders` orders, each placed by one person and drawn beside them; `tags` tags
 * with no position, a third of the people tagged with one.
 */
export function write(dir, { count = 70_000, clusters = 256, orders, tags = 16 } = {}) {
  orders ??= Math.ceil(count / 4);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, "vertex"), { recursive: true });
  mkdirSync(join(dir, "edge"), { recursive: true });

  const { points: people, per } = positions(count, clusters);
  const placedBy = (j) => (j * 7919) % count;
  const orderPoints = Array.from({ length: orders }, (_, j) => {
    const p = people[placedBy(j)];
    return { cluster: p.cluster, x: Math.fround(p.x + 1.5), y: Math.fround(p.y - 1.5) };
  });

  // The union's extent, and every drawn vertex's rank on the curve over it: that rank IS the id.
  const extent = { minX: Infinity, maxX: -Infinity, minY: Infinity, maxY: -Infinity };
  for (const p of [...people, ...orderPoints]) {
    extent.minX = Math.min(extent.minX, p.x);
    extent.maxX = Math.max(extent.maxX, p.x);
    extent.minY = Math.min(extent.minY, p.y);
    extent.maxY = Math.max(extent.maxY, p.y);
  }
  const placed = [
    ...people.map((p, i) => ({ type: 0, i, p })),
    ...orderPoints.map((p, i) => ({ type: 1, i, p })),
  ].map((v) => ({
    ...v,
    code: hilbert2(quantize(v.p.x, extent.minX, extent.maxX), quantize(v.p.y, extent.minY, extent.maxY)),
  }));
  placed.sort((a, b) => a.code - b.code || a.type - b.type || a.i - b.i);
  const personId = new Uint32Array(count);
  const orderId = new Uint32Array(orders);
  placed.forEach((v, dense) => ((v.type === 0 ? personId : orderId)[v.i] = dense));
  // The undrawn type takes the ids after every drawn one, in subject order.
  const tagSubjects = Array.from({ length: tags }, (_, k) => `${BASE}tag/${k}`);
  const tagOrder = tagSubjects.map((s, k) => ({ s, k })).sort((a, b) => (a.s < b.s ? -1 : a.s > b.s ? 1 : 0));
  const tagId = new Uint32Array(tags);
  tagOrder.forEach(({ k }, rank) => (tagId[k] = placed.length + rank));

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
    const start = people[i].cluster * per;
    const size = Math.min(per, count - start);
    link(i, start + ((i - start) * 7 + 3) % size);
  }

  const scratch = join(dir, ".csv");
  mkdirSync(scratch, { recursive: true });
  const at = (name) => join(scratch, `${name}.csv`);
  csv(at("Person"), "dense_id,subject,x,y,cluster_id,birth_year,postcode",
    people.map((p, i) => [personId[i], `${BASE}person/${i}`, p.x, p.y, p.cluster,
      i % 13 === 0 ? null : 1940 + (i % 70), `PC${String(i % 997).padStart(4, "0")}`]));
  csv(at("Order"), "dense_id,subject,x,y,cluster_id,amount",
    orderPoints.map((p, j) => [orderId[j], `${BASE}order/${j}`, p.x, p.y, p.cluster, ((j * 37) % 1000) / 4]));
  csv(at("Tag"), "dense_id,subject,name", tagSubjects.map((s, k) => [tagId[k], s, `tag-${k}`]));
  csv(at("knows"), "src,dst,since", pairs.map(([a, b]) => [personId[a], personId[b], 2000 + ((a + b) % 25)]));
  csv(at("placedBy"), "src,dst", Array.from({ length: orders }, (_, j) => [orderId[j], personId[placedBy(j)]]));
  const tagged = [];
  for (let i = 0; i < count; i += 3) if (tags > 0) tagged.push([personId[i], tagId[i % tags]]);
  csv(at("tagged"), "src,dst", tagged);

  const P = (name, type, extra = {}) => ({ name, type, ...extra });
  const drawn = [P("dense_id", "uint32"), P("subject", "string"), P("x", "float"), P("y", "float"), P("cluster_id", "uint32")];
  const vertexTables = [
    {
      name: "Person", iri: `${BASE}Person`, rows: count, sql: "dense_id::UINTEGER AS dense_id, subject::VARCHAR AS subject, x::FLOAT AS x, y::FLOAT AS y, cluster_id::UINTEGER AS cluster_id, birth_year::INTEGER AS birth_year, postcode::VARCHAR AS postcode",
      properties: [...drawn, P("birth_year", "int32", { iri: `${BASE}birthYear`, nullable: true }), P("postcode", "string", { iri: `${BASE}postcode` })],
      position: true,
    },
    {
      name: "Order", iri: `${BASE}Order`, rows: orders, sql: "dense_id::UINTEGER AS dense_id, subject::VARCHAR AS subject, x::FLOAT AS x, y::FLOAT AS y, cluster_id::UINTEGER AS cluster_id, amount::DOUBLE AS amount",
      properties: [...drawn, P("amount", "double", { iri: `${BASE}amount` })],
      position: true,
    },
    {
      name: "Tag", iri: `${BASE}Tag`, rows: tags, sql: "dense_id::UINTEGER AS dense_id, subject::VARCHAR AS subject, name::VARCHAR AS name",
      properties: [P("dense_id", "uint32"), P("subject", "string"), P("name", "string", { iri: `${BASE}name` })],
      position: false,
    },
  ].filter((t) => t.rows > 0);
  const ends = [P("src", "uint32"), P("dst", "uint32")];
  const edgeTables = [
    { src: "Person", label: "knows", dst: "Person", csv: "knows", rows: pairs.length, sql: "src::UINTEGER AS src, dst::UINTEGER AS dst, since::INTEGER AS since", properties: [...ends, P("since", "int32")] },
    { src: "Order", label: "placedBy", dst: "Person", csv: "placedBy", rows: orders, sql: "src::UINTEGER AS src, dst::UINTEGER AS dst", properties: ends },
    { src: "Person", label: "tagged", dst: "Tag", csv: "tagged", rows: tagged.length, sql: "src::UINTEGER AS src, dst::UINTEGER AS dst", properties: ends },
  ].filter((e) => e.rows > 0);

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
      ...(t.position ? { position: { by: "layout", x: "x", y: "y" } } : {}),
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
  };
  writeFileSync(join(dir, ENTRY_POINT), `${JSON.stringify(manifest, null, 2)}\n`);

  return {
    dir,
    vertices: vertexTables.reduce((n, t) => n + t.rows, 0),
    placed: placed.length,
    edges: edgeTables.reduce((n, e) => n + e.rows, 0),
    tables: vertexTables.length + edgeTables.length,
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [dir] = process.argv.slice(2).filter((a, i, all) => !a.startsWith("--") && !all[i - 1]?.startsWith("--"));
  if (!dir) {
    console.error("usage: node guards/fixture.mjs <dir> [--vertices N] [--clusters N] [--orders N] [--tags N]");
    process.exit(2);
  }
  const flag = (name) => {
    const at = process.argv.indexOf(`--${name}`);
    return at === -1 ? undefined : Number(process.argv[at + 1]);
  };
  const written = write(dir, {
    count: flag("vertices") ?? 70_000,
    clusters: flag("clusters") ?? 256,
    orders: flag("orders"),
    tags: flag("tags") ?? 16,
  });
  console.log(
    `${written.vertices} vertices (${written.placed} drawn) · ${written.edges} edges · ` +
      `${written.tables} tables → ${written.dir}`,
  );
}
