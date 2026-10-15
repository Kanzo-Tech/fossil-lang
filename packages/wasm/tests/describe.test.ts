import "./boot.js";

import type { Access, Input, Scope, StorageCredential, Table } from "@fossil-lang/types";
import { describe, expect, it, vi } from "vitest";
import { profile, type ProfileIO } from "../src/index.js";

/** Rows as the columns an engine answers in. */
function tableOf(rows: readonly Record<string, unknown>[]): Table {
  const names = [...new Set(rows.flatMap((row) => Object.keys(row)))];
  return {
    numRows: rows.length,
    schema: { fields: names.map((name) => ({ name })) },
    getChild: (name) =>
      names.includes(name)
        ? {
            length: rows.length,
            get: (i) => rows[i]?.[name] ?? null,
            toArray: () => rows.map((row) => row[name]),
          }
        : null,
  };
}

describe("profile", () => {
  const W = "s3://bucket/w/";
  const users: Input = {
    role: "data",
    binding: "users",
    key: "@w/users.csv",
    location: `${W}users.csv`,
    connection: "w",
    format: "csv",
  };
  const orders: Input = {
    role: "data",
    binding: "orders",
    key: "@w/orders.csv",
    location: `${W}orders.csv`,
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
    overrides: Partial<ProfileIO> = {},
    vend: (scope: Scope) => StorageCredential[] = () => [credential(W)],
  ) {
    const sql: string[] = [];
    const credentials = vi.fn(async (scope: Scope, _access: Access) => vend(scope));
    const io: ProfileIO = {
      host: { connections: async () => ({ w: W }), credentials },
      engine: {
        query: async (text) => {
          if (text.includes("duckdb_extensions()")) return tableOf([{ loaded: true }]);
          sql.push(text);
          return tableOf(text.startsWith("DESCRIBE") ? await query(text) : []);
        },
        registerFiles: async () => {},
        dropFiles: async () => {},
        registerFileBuffer: async () => {},
      },
      options: { depth: "types" },
      ...overrides,
    };
    return { io, credentials, sql };
  }

  const describes = (sql: string[]) => sql.filter((s) => s.startsWith("DESCRIBE"));

  it("mounts each connection once, describes the s3:// location and drops the secret after", async () => {
    const { io, credentials, sql } = fakeIO(async (text) =>
      text.includes("users")
        ? [{ column_name: "id", column_type: "BIGINT" }]
        : [{ column_name: "total", column_type: "DOUBLE" }],
    );

    const { descriptors } = await profile([users, orders], io);

    expect(credentials).toHaveBeenCalledTimes(1);
    expect(credentials).toHaveBeenCalledWith({ connection: "w" }, "read", { signal: expect.any(AbortSignal) });
    expect(sql[0]).toMatch(/^CREATE OR REPLACE SECRET fossil_read_[0-9a-f]{16} \(TYPE s3, /);
    expect(describes(sql)).toEqual([
      `DESCRIBE SELECT * FROM read_csv_auto('${W}users.csv')`,
      `DESCRIBE SELECT * FROM read_csv_auto('${W}orders.csv')`,
    ]);
    expect(sql.at(-1)).toMatch(/^DROP SECRET IF EXISTS fossil_read_[0-9a-f]{16}$/);
    expect(descriptors).toEqual([
      {
        key: "@w/users.csv",
        columns: [{ name: "id", primitive: "integer" }],
        etag: "",
      },
      {
        key: "@w/orders.csv",
        columns: [{ name: "total", primitive: "float" }],
        etag: "",
      },
    ]);
  });

  it("describes a public source with no connection as it is, asking the host nothing", async () => {
    const { io, credentials, sql } = fakeIO(async () => [{ column_name: "id", column_type: "INT" }]);
    const { descriptors } = await profile(
      [{ role: "data", binding: "p", key: "https://x.test/p.csv", location: "https://x.test/p.csv", format: "csv" }],
      io,
    );
    expect(credentials).not.toHaveBeenCalled();
    expect(sql).toEqual(["DESCRIBE SELECT * FROM read_csv_auto('https://x.test/p.csv')"]);
    expect(descriptors).toHaveLength(1);
  });

  it("describes a parquet source as parquet, and carries a csv source's delimiter", async () => {
    const { io, sql } = fakeIO(async () => [{ column_name: "ts", column_type: "TIMESTAMP" }]);
    await profile(
      [
        { role: "data", binding: "e", key: "@w/e.parquet", location: `${W}e.parquet`, connection: "w", format: "parquet" },
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
    const { descriptors } = await profile(
      [{ role: "data", binding: "g", key: "@w/g.ttl", location: `${W}g.ttl`, connection: "w", format: "rdf" }],
      io,
    );
    expect(descriptors).toEqual([]);
    expect(credentials).not.toHaveBeenCalled();
    expect(sql).toEqual([]);
  });

  it("runs the whole plan when no depth is asked for: the counts after the DESCRIBEs", async () => {
    const { io, sql } = fakeIO(async () => [{ column_name: "id", column_type: "BIGINT" }], { options: undefined });
    const { profile: profiled } = await profile([users], io);
    expect(sql.some((s) => s.startsWith("SELECT CAST(count(*) AS VARCHAR)"))).toBe(true);
    expect(profiled.sources.map((s) => [s.binding, s.descriptor.key])).toEqual([["users", "@w/users.csv"]]);
  });

  it("asks the host for an etag per source", async () => {
    const etag = vi.fn((source: Input) => `etag-for-${source.key}`);
    const { io } = fakeIO(async () => [{ column_name: "id", column_type: "INT" }], { etag });
    const { descriptors } = await profile([users], io);
    expect(etag).toHaveBeenCalledWith(users);
    expect(descriptors[0]?.etag).toBe("etag-for-@w/users.csv");
  });

  it("is best-effort: an unvended, unaddressable or unreadable source is answered with its problem and skipped", async () => {
    const { io, sql } = fakeIO(
      async (text) => {
        if (text.includes("orders")) throw new Error("CORS / unreachable");
        return [{ column_name: "total", column_type: "INT" }];
      },
      {},
      (scope) => ("connection" in scope && scope.connection === "gone" ? [] : [credential(W)]),
    );
    const parquet: Input = {
      role: "data",
      binding: "e",
      key: "@w/e.parquet",
      location: `${W}e.parquet`,
      connection: "w",
      format: "parquet",
    };
    const unvended: Input = { ...users, binding: "u", connection: "gone" };
    const bare: Input = { role: "data", binding: "b", key: "b.csv", location: "b.csv", format: "csv" };

    const { descriptors, undescribed } = await profile([unvended, orders, parquet, bare], io);

    expect(undescribed.map((u) => [u.source.binding, u.problem.code])).toEqual([
      ["u", "storage/no-credential"],
      [orders.binding, "engine/failed"],
      ["b", "storage/no-route"],
    ]);
    expect(undescribed[1]!.problem.cause).toEqual({ name: "Error", detail: "CORS / unreachable" });
    expect(descriptors).toEqual([
      {
        key: "@w/e.parquet",
        columns: [{ name: "total", primitive: "integer" }],
        etag: "",
      },
    ]);
    expect(sql.at(-1)).toMatch(/^DROP SECRET IF EXISTS /);
  });

  it("answers a host that never vends as storage/host-silent after 30 s, not a stall", async () => {
    vi.useFakeTimers();
    try {
      const { io } = fakeIO(async () => [], {
        host: { connections: async () => ({}), credentials: () => new Promise(() => {}) },
      });
      const outcome = profile([users], io);
      await vi.advanceTimersByTimeAsync(30_000);
      const { undescribed } = await outcome;
      expect(undescribed.map((u) => u.problem.code)).toEqual(["storage/host-silent"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops a DESCRIBE that hangs when the caller does, and still gives the credential back", async () => {
    const stop = new AbortController();
    const { io, sql } = fakeIO(() => new Promise(() => {}), { signal: stop.signal });
    io.engine = {
      ...io.engine,
      query: (text, options) =>
        text.startsWith("DESCRIBE")
          ? new Promise((_, reject) => {
              options.signal.addEventListener("abort", () => reject(options.signal.reason));
              stop.abort(new DOMException("stopped", "AbortError"));
            })
          : (sql.push(text), Promise.resolve(tableOf(text.includes("duckdb_extensions()") ? [{ loaded: true }] : []))),
    };
    await expect(profile([users], io)).rejects.toMatchObject({ name: "AbortError" });
    expect(sql.at(-1)).toMatch(/^DROP SECRET IF EXISTS /);
  });
});
