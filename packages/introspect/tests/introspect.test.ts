import type { ProgramSource } from "@fossil-lang/types";
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
  const users: ProgramSource = {
    binding: "users",
    key: "@w/users.csv",
    locator: "s3://bucket/w/users.csv",
    format: "csv",
  };
  const orders: ProgramSource = {
    binding: "orders",
    key: "@w/orders.csv",
    locator: "s3://bucket/w/orders.csv",
    format: "csv",
  };

  /** A host that signs every locator it is given, and a DuckDB that records
   *  what was registered under which name. */
  function fakeIO(
    query: IntrospectIO["query"],
    overrides: Partial<IntrospectIO> = {},
  ) {
    const registered = new Map<string, string>();
    const sign = vi.fn(async (locators: string[]) =>
      Object.fromEntries(locators.map((l) => [l, `https://signed/${l}`])),
    );
    const io: IntrospectIO = {
      host: { connections: async () => ({}), sign },
      register: async (name, url) => {
        registered.set(name, url);
      },
      query,
      ...overrides,
    };
    return { io, sign, registered };
  }

  it("signs every locator in one call, registers each under its key and describes the key", async () => {
    const seen: string[] = [];
    const { io, sign, registered } = fakeIO(async (sql) => {
      seen.push(sql);
      return sql.includes("users")
        ? [{ column_name: "id", column_type: "BIGINT" }]
        : [{ column_name: "total", column_type: "DOUBLE" }];
    });

    const descriptors = await introspect([users, orders], io);

    expect(sign).toHaveBeenCalledTimes(1);
    expect(sign).toHaveBeenCalledWith([users.locator, orders.locator]);
    expect(registered).toEqual(
      new Map([
        ["@w/users.csv", "https://signed/s3://bucket/w/users.csv"],
        ["@w/orders.csv", "https://signed/s3://bucket/w/orders.csv"],
      ]),
    );
    expect(seen).toEqual([
      "DESCRIBE SELECT * FROM read_csv_auto('@w/users.csv')",
      "DESCRIBE SELECT * FROM read_csv_auto('@w/orders.csv')",
    ]);
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

  it("describes a parquet source as parquet, and carries a csv source's delimiter", async () => {
    const seen: string[] = [];
    const { io } = fakeIO(async (sql) => {
      seen.push(sql);
      return [{ column_name: "ts", column_type: "TIMESTAMP" }];
    });
    await introspect(
      [
        { binding: "e", key: "@w/e.parquet", locator: "s3://b/e.parquet", format: "parquet" },
        { ...users, option: "|" },
      ],
      io,
    );
    expect(seen).toEqual([
      "DESCRIBE SELECT * FROM read_parquet('@w/e.parquet')",
      "DESCRIBE SELECT * FROM read_csv_auto('@w/users.csv', delim='|')",
    ]);
  });

  it("does not describe a materialised source, and asks the host nothing for none", async () => {
    const query = vi.fn(async () => []);
    const { io, sign } = fakeIO(query);
    const descriptors = await introspect(
      [{ binding: "g", key: "@w/g.ttl", locator: "s3://b/g.ttl", format: "rdf" }],
      io,
    );
    expect(descriptors).toEqual([]);
    expect(sign).not.toHaveBeenCalled();
    expect(query).not.toHaveBeenCalled();
  });

  it("asks the host for a freshness token against the signed url", async () => {
    const freshness = vi.fn((_: ProgramSource, url: string) => `etag-for-${url}`);
    const { io } = fakeIO(async () => [{ column_name: "id", column_type: "INT" }], {
      freshness,
    });
    const descriptors = await introspect([users], io);
    expect(freshness).toHaveBeenCalledWith(users, "https://signed/s3://bucket/w/users.csv");
    expect(descriptors[0]?.freshness_token).toBe(
      "etag-for-https://signed/s3://bucket/w/users.csv",
    );
  });

  it("is best-effort: an unsigned or unreadable source is reported and skipped", async () => {
    const onWarn = vi.fn();
    const { io } = fakeIO(
      async (sql) => {
        if (sql.includes("orders")) throw new Error("CORS / unreachable");
        return [{ column_name: "total", column_type: "INT" }];
      },
      {
        host: {
          connections: async () => ({}),
          sign: async (locators) =>
            Object.fromEntries(
              locators.filter((l) => l !== users.locator).map((l) => [l, `https://signed/${l}`]),
            ),
        },
        onWarn,
      },
    );
    const parquet: ProgramSource = {
      binding: "e",
      key: "@w/e.parquet",
      locator: "s3://b/e.parquet",
      format: "parquet",
    };

    const descriptors = await introspect([users, orders, parquet], io);

    expect(onWarn).toHaveBeenCalledTimes(2);
    expect(descriptors).toEqual([
      {
        uri: "@w/e.parquet",
        columns: [{ name: "total", primitive: "integer" }],
        freshness_token: "",
      },
    ]);
  });
});
