import "./boot.js";

import type { Access, ProgramSource, Scope, StorageCredential } from "@fossil-lang/types";
import { describe, expect, it, vi } from "vitest";
import {
  buildDescriptor,
  describeSql,
  duckdbTypeToFossilPrimitive,
  introspect,
  type DescribeRow,
  type IntrospectIO,
} from "../src/index.js";

describe("duckdbTypeToFossilPrimitive", () => {
  it("maps integer family to integer", () => {
    for (const t of ["INTEGER", "BIGINT", "INT", "SMALLINT", "TINYINT", "HUGEINT"]) {
      expect(duckdbTypeToFossilPrimitive(t)).toBe("integer");
    }
  });

  it("maps float family + DECIMAL(p,s) to Float", () => {
    for (const t of ["DOUBLE", "FLOAT", "REAL", "DECIMAL(10,2)"]) {
      expect(duckdbTypeToFossilPrimitive(t)).toBe("float");
    }
  });

  it("maps temporal + boolean types", () => {
    expect(duckdbTypeToFossilPrimitive("BOOLEAN")).toBe("bool");
    expect(duckdbTypeToFossilPrimitive("DATE")).toBe("date");
    expect(duckdbTypeToFossilPrimitive("TIMESTAMP")).toBe("date_time");
    expect(duckdbTypeToFossilPrimitive("DATETIME")).toBe("date_time");
    expect(duckdbTypeToFossilPrimitive("TIME")).toBe("time");
  });

  it("falls back to String for VARCHAR + unknown types, case/space-insensitive", () => {
    expect(duckdbTypeToFossilPrimitive("VARCHAR")).toBe("string");
    expect(duckdbTypeToFossilPrimitive("  text ")).toBe("string");
    expect(duckdbTypeToFossilPrimitive("STRUCT(a INT)")).toBe("string");
  });
});

describe("describeSql", () => {
  it("single-quote-escapes the url", () => {
    expect(describeSql("o'brien.csv", "csv")).toBe(
      "DESCRIBE SELECT * FROM read_csv_auto('o''brien.csv')",
    );
  });

  it("carries the reader option under DuckDB's name for it", () => {
    expect(describeSql("u.csv", "csv", "|")).toBe(
      "DESCRIBE SELECT * FROM read_csv_auto('u.csv', delim='|')",
    );
    expect(describeSql("u.json", "json", "|")).toBe(
      "DESCRIBE SELECT * FROM read_json_auto('u.json')",
    );
  });

  it("reads through the table function the constructor names", () => {
    expect(describeSql("https://x/u.csv", "csv")).toBe(
      "DESCRIBE SELECT * FROM read_csv_auto('https://x/u.csv')",
    );
    expect(describeSql("https://x/u.json", "json")).toBe(
      "DESCRIBE SELECT * FROM read_json_auto('https://x/u.json')",
    );
    expect(describeSql("https://x/u.parquet", "parquet")).toBe(
      "DESCRIBE SELECT * FROM read_parquet('https://x/u.parquet')",
    );
  });
});

describe("buildDescriptor", () => {
  it("maps DESCRIBE rows to a descriptor keyed by URI, with no token", () => {
    const rows: DescribeRow[] = [
      { column_name: "id", column_type: "INTEGER" },
      { column_name: "name", column_type: "VARCHAR" },
      { column_name: "joined", column_type: "TIMESTAMP" },
    ];
    expect(buildDescriptor("data/users.csv", rows)).toEqual({
      uri: "data/users.csv",
      columns: [
        { name: "id", primitive: "integer" },
        { name: "name", primitive: "string" },
        { name: "joined", primitive: "date_time" },
      ],
      freshness_token: "",
    });
  });

  it("carries the host's freshness token through when it supplies one", () => {
    const rows: DescribeRow[] = [{ column_name: "id", column_type: "INT" }];
    expect(buildDescriptor("u.csv", rows, 'W/"abc"').freshness_token).toBe(
      'W/"abc"',
    );
  });

  it("drops columns with empty/missing names", () => {
    const rows: DescribeRow[] = [
      { column_name: "ok", column_type: "INT" },
      { column_name: "", column_type: "INT" },
      { column_type: "INT" },
    ];
    expect(buildDescriptor("s.csv", rows).columns).toEqual([
      { name: "ok", primitive: "integer" },
    ]);
  });
});

describe("introspect", () => {
  const W = "s3://bucket/w/";
  const users: ProgramSource = {
    binding: "users",
    key: "@w/users.csv",
    locator: `${W}users.csv`,
    connection: "w",
    format: "csv",
  };
  const orders: ProgramSource = {
    binding: "orders",
    key: "@w/orders.csv",
    locator: `${W}orders.csv`,
    connection: "w",
    format: "csv",
  };

  const credential = (prefix: string): StorageCredential => ({
    prefix,
    config: {
      "s3.access-key-id": "K",
      "s3.secret-access-key": "secret",
      "s3.endpoint": "http://localhost:9000",
      "s3.path-style-access": "true",
      "client.region": "us-east-1",
    },
  });

  /** A host that vends a read credential per connection, and an engine that
   *  records every statement but the `httpfs` probe, which it answers as loaded. */
  function fakeIO(
    query: (sql: string) => Promise<Record<string, unknown>[]>,
    overrides: Partial<IntrospectIO> = {},
    vend: (scope: Scope) => StorageCredential[] = () => [credential(W)],
  ) {
    const sql: string[] = [];
    const credentials = vi.fn(async (scope: Scope, _access: Access) => vend(scope));
    const io: IntrospectIO = {
      host: { connections: async () => ({ w: W }), credentials },
      engine: {
        query: async (text) => {
          if (text.includes("duckdb_extensions()")) return [{ loaded: true }];
          sql.push(text);
          return text.startsWith("DESCRIBE") ? query(text) : [];
        },
        lend: async () => {},
        drop: async () => {},
      },
      ...overrides,
    };
    return { io, credentials, sql };
  }

  const describes = (sql: string[]) => sql.filter((s) => s.startsWith("DESCRIBE"));

  it("mounts each connection once, describes the s3:// locator and drops the secret after", async () => {
    const { io, credentials, sql } = fakeIO(async (text) =>
      text.includes("users")
        ? [{ column_name: "id", column_type: "BIGINT" }]
        : [{ column_name: "total", column_type: "DOUBLE" }],
    );

    const descriptors = await introspect([users, orders], io);

    expect(credentials).toHaveBeenCalledTimes(1);
    expect(credentials).toHaveBeenCalledWith({ connection: "w" }, "read");
    expect(sql[0]).toMatch(/^CREATE OR REPLACE SECRET fossil_read_[0-9a-f]{16} \(TYPE s3, /);
    expect(describes(sql)).toEqual([
      `DESCRIBE SELECT * FROM read_csv_auto('${W}users.csv')`,
      `DESCRIBE SELECT * FROM read_csv_auto('${W}orders.csv')`,
    ]);
    expect(sql.at(-1)).toMatch(/^DROP SECRET IF EXISTS fossil_read_[0-9a-f]{16}$/);
    expect(descriptors).toEqual([
      {
        uri: "@w/users.csv",
        columns: [{ name: "id", primitive: "integer" }],
        freshness_token: "",
      },
      {
        uri: "@w/orders.csv",
        columns: [{ name: "total", primitive: "float" }],
        freshness_token: "",
      },
    ]);
  });

  it("describes a public source with no connection as it is, asking the host nothing", async () => {
    const { io, credentials, sql } = fakeIO(async () => [{ column_name: "id", column_type: "INT" }]);
    const descriptors = await introspect(
      [{ binding: "p", key: "https://x.test/p.csv", locator: "https://x.test/p.csv", format: "csv" }],
      io,
    );
    expect(credentials).not.toHaveBeenCalled();
    expect(sql).toEqual(["DESCRIBE SELECT * FROM read_csv_auto('https://x.test/p.csv')"]);
    expect(descriptors).toHaveLength(1);
  });

  it("describes a parquet source as parquet, and carries a csv source's delimiter", async () => {
    const { io, sql } = fakeIO(async () => [{ column_name: "ts", column_type: "TIMESTAMP" }]);
    await introspect(
      [
        { binding: "e", key: "@w/e.parquet", locator: `${W}e.parquet`, connection: "w", format: "parquet" },
        { ...users, option: "|" },
      ],
      io,
    );
    expect(describes(sql)).toEqual([
      `DESCRIBE SELECT * FROM read_parquet('${W}e.parquet')`,
      `DESCRIBE SELECT * FROM read_csv_auto('${W}users.csv', delim='|')`,
    ]);
  });

  it("does not describe a materialised source, and asks the host nothing for none", async () => {
    const query = vi.fn(async () => []);
    const { io, credentials, sql } = fakeIO(query);
    const descriptors = await introspect(
      [{ binding: "g", key: "@w/g.ttl", locator: `${W}g.ttl`, connection: "w", format: "rdf" }],
      io,
    );
    expect(descriptors).toEqual([]);
    expect(credentials).not.toHaveBeenCalled();
    expect(sql).toEqual([]);
  });

  it("asks the host for a freshness token per source", async () => {
    const freshness = vi.fn((source: ProgramSource) => `etag-for-${source.key}`);
    const { io } = fakeIO(async () => [{ column_name: "id", column_type: "INT" }], { freshness });
    const descriptors = await introspect([users], io);
    expect(freshness).toHaveBeenCalledWith(users);
    expect(descriptors[0]?.freshness_token).toBe("etag-for-@w/users.csv");
  });

  it("is best-effort: an unvended, unaddressable or unreadable source is reported and skipped", async () => {
    const onWarn = vi.fn();
    const { io, sql } = fakeIO(
      async (text) => {
        if (text.includes("orders")) throw new Error("CORS / unreachable");
        return [{ column_name: "total", column_type: "INT" }];
      },
      { onWarn },
      (scope) => ("connection" in scope && scope.connection === "gone" ? [] : [credential(W)]),
    );
    const parquet: ProgramSource = {
      binding: "e",
      key: "@w/e.parquet",
      locator: `${W}e.parquet`,
      connection: "w",
      format: "parquet",
    };
    const unvended: ProgramSource = { ...users, binding: "u", connection: "gone" };
    const bare: ProgramSource = { binding: "b", key: "b.csv", locator: "b.csv", format: "csv" };

    const descriptors = await introspect([unvended, orders, parquet, bare], io);

    expect(onWarn).toHaveBeenCalledTimes(3);
    expect(descriptors).toEqual([
      {
        uri: "@w/e.parquet",
        columns: [{ name: "total", primitive: "integer" }],
        freshness_token: "",
      },
    ]);
    expect(sql.at(-1)).toMatch(/^DROP SECRET IF EXISTS /);
  });
});

