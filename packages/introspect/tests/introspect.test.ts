import { describe, expect, it, vi } from "vitest";
import {
  buildDescriptor,
  describeSql,
  duckdbTypeToFossilPrimitive,
  extractSourceRefs,
  introspect,
  type DescribeRow,
  type SourceRef,
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

describe("extractSourceRefs", () => {
  it("scrapes every constructor, with the @conn/path form, keeping the one written", () => {
    const text = [
      'users := io.csv("@warehouse/users.csv")',
      "orders := io.json('@warehouse/orders.json')",
      'events := io.parquet("@warehouse/events.parquet")',
    ].join("\n");
    expect(extractSourceRefs(text)).toEqual([
      { sourceName: "users", format: "csv", url: "@warehouse/users.csv" },
      { sourceName: "orders", format: "json", url: "@warehouse/orders.json" },
      { sourceName: "events", format: "parquet", url: "@warehouse/events.parquet" },
    ]);
  });

  it("ignores non-source lines and tolerates surrounding whitespace", () => {
    const text = '  people  :=  io.csv( "data.csv" )\nx := 1 + 2\n';
    expect(extractSourceRefs(text)).toEqual([
      { sourceName: "people", format: "csv", url: "data.csv" },
    ]);
  });

  it("returns empty for text with no source bindings", () => {
    expect(extractSourceRefs("User : Person from users")).toEqual([]);
  });
});

describe("describeSql", () => {
  it("single-quote-escapes the url", () => {
    expect(describeSql("o'brien.csv", "csv")).toBe(
      "DESCRIBE SELECT * FROM read_csv_auto('o''brien.csv')",
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
  const mapping = [
    'users := io.csv("@w/users.csv")',
    'orders := io.csv("@w/orders.csv")',
  ].join("\n");

  it("resolves + queries each source and returns the descriptors", async () => {
    const resolve = (ref: SourceRef) => `https://signed/${ref.url}`;
    const query = vi.fn(async (sql: string): Promise<DescribeRow[]> => {
      if (sql.includes("users")) {
        return [{ column_name: "id", column_type: "BIGINT" }];
      }
      return [{ column_name: "total", column_type: "DOUBLE" }];
    });

    const descriptors = await introspect(mapping, { resolve, query });

    expect(query).toHaveBeenCalledTimes(2);
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

  it("introspects a parquet binding, and asks DuckDB for it as parquet", async () => {
    const seen: string[] = [];
    const descriptors = await introspect('e := io.parquet("@w/events.parquet")', {
      resolve: (r) => r.url,
      query: async (sql) => {
        seen.push(sql);
        return [{ column_name: "ts", column_type: "TIMESTAMP" }];
      },
    });
    expect(seen).toEqual([
      "DESCRIBE SELECT * FROM read_parquet('@w/events.parquet')",
    ]);
    expect(descriptors).toEqual([
      {
        uri: "@w/events.parquet",
        columns: [{ name: "ts", primitive: "date_time" }],
        freshness_token: "",
      },
    ]);
  });

  it("asks the host for a freshness token and stamps it on the descriptor", async () => {
    const freshness = vi.fn((ref: SourceRef) => `etag-for-${ref.url}`);
    const descriptors = await introspect('u := io.csv("@w/u.csv")', {
      resolve: (r) => r.url,
      query: async () => [{ column_name: "id", column_type: "INT" }],
      freshness,
    });
    expect(freshness).toHaveBeenCalledTimes(1);
    expect(descriptors[0]?.freshness_token).toBe("etag-for-@w/u.csv");
  });

  it("is best-effort: a failing source is skipped (logged), the rest succeed", async () => {
    const resolve = (ref: SourceRef) => ref.url;
    const query = async (sql: string): Promise<DescribeRow[]> => {
      if (sql.includes("users")) throw new Error("CORS / unreachable");
      return [{ column_name: "total", column_type: "INT" }];
    };
    const onWarn = vi.fn();

    const descriptors = await introspect(mapping, { resolve, query, onWarn });

    expect(onWarn).toHaveBeenCalledTimes(1);
    expect(descriptors).toEqual([
      {
        uri: "@w/orders.csv",
        columns: [{ name: "total", primitive: "integer" }],
        freshness_token: "",
      },
    ]);
  });

  it("returns empty for a mapping with no source bindings", async () => {
    const descriptors = await introspect("x := 1 + 2", {
      resolve: (r) => r.url,
      query: async () => [],
    });
    expect(descriptors).toEqual([]);
  });
});
