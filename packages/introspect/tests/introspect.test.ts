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
  it("maps integer family to Integer", () => {
    for (const t of ["INTEGER", "BIGINT", "INT", "SMALLINT", "TINYINT", "HUGEINT"]) {
      expect(duckdbTypeToFossilPrimitive(t)).toBe("Integer");
    }
  });

  it("maps float family + DECIMAL(p,s) to Float", () => {
    for (const t of ["DOUBLE", "FLOAT", "REAL", "DECIMAL(10,2)"]) {
      expect(duckdbTypeToFossilPrimitive(t)).toBe("Float");
    }
  });

  it("maps temporal + boolean types", () => {
    expect(duckdbTypeToFossilPrimitive("BOOLEAN")).toBe("Bool");
    expect(duckdbTypeToFossilPrimitive("DATE")).toBe("Date");
    expect(duckdbTypeToFossilPrimitive("TIMESTAMP")).toBe("DateTime");
    expect(duckdbTypeToFossilPrimitive("DATETIME")).toBe("DateTime");
    expect(duckdbTypeToFossilPrimitive("TIME")).toBe("Time");
  });

  it("falls back to String for VARCHAR + unknown types, case/space-insensitive", () => {
    expect(duckdbTypeToFossilPrimitive("VARCHAR")).toBe("String");
    expect(duckdbTypeToFossilPrimitive("  text ")).toBe("String");
    expect(duckdbTypeToFossilPrimitive("STRUCT(a INT)")).toBe("String");
  });
});

describe("extractSourceRefs", () => {
  it("scrapes csv + json source bindings with the @conn/path form", () => {
    const text = [
      'users := io.csv("@warehouse/users.csv")',
      "orders := io.json('@warehouse/orders.json')",
    ].join("\n");
    expect(extractSourceRefs(text)).toEqual([
      { sourceName: "users", url: "@warehouse/users.csv" },
      { sourceName: "orders", url: "@warehouse/orders.json" },
    ]);
  });

  it("ignores non-source lines and tolerates surrounding whitespace", () => {
    const text = '  people  :=  io.csv( "data.csv" )\nx := 1 + 2\n';
    expect(extractSourceRefs(text)).toEqual([
      { sourceName: "people", url: "data.csv" },
    ]);
  });

  it("returns empty for text with no source bindings", () => {
    expect(extractSourceRefs("prefix ex: <https://example.org/>")).toEqual([]);
  });
});

describe("describeSql", () => {
  it("wraps the url in read_csv_auto and single-quote-escapes it", () => {
    expect(describeSql("https://x/u.csv")).toBe(
      "DESCRIBE SELECT * FROM read_csv_auto('https://x/u.csv')",
    );
    expect(describeSql("o'brien.csv")).toBe(
      "DESCRIBE SELECT * FROM read_csv_auto('o''brien.csv')",
    );
  });
});

describe("buildDescriptor", () => {
  it("maps DESCRIBE rows to a typed descriptor with empty content_hash", () => {
    const rows: DescribeRow[] = [
      { column_name: "id", column_type: "INTEGER" },
      { column_name: "name", column_type: "VARCHAR" },
      { column_name: "joined", column_type: "TIMESTAMP" },
    ];
    expect(buildDescriptor("users", rows)).toEqual({
      source_name: "users",
      columns: [
        { name: "id", primitive: "Integer" },
        { name: "name", primitive: "String" },
        { name: "joined", primitive: "DateTime" },
      ],
      content_hash: "",
    });
  });

  it("drops columns with empty/missing names", () => {
    const rows: DescribeRow[] = [
      { column_name: "ok", column_type: "INT" },
      { column_name: "", column_type: "INT" },
      { column_type: "INT" },
    ];
    expect(buildDescriptor("s", rows).columns).toEqual([
      { name: "ok", primitive: "Integer" },
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
        source_name: "users",
        columns: [{ name: "id", primitive: "Integer" }],
        content_hash: "",
      },
      {
        source_name: "orders",
        columns: [{ name: "total", primitive: "Float" }],
        content_hash: "",
      },
    ]);
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
        source_name: "orders",
        columns: [{ name: "total", primitive: "Integer" }],
        content_hash: "",
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
