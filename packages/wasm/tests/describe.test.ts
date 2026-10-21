import "./boot.js";

import type { Access, DataType, Input, Scope, StorageCredential } from "@fossil-lang/types";
import { describe, expect, it, vi } from "vitest";
import { profile, type ProfileIO } from "../src/index.js";
import { ARROW, asksColumns, columnsOf, tableOf } from "./host.js";

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
    query: (sql: string) => Promise<Record<string, DataType>>,
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
          return asksColumns(text) ? columnsOf(await query(text)) : tableOf([]);
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

  const describes = (sql: string[]) => sql.filter(asksColumns);

  it("mounts each connection once, describes the s3:// location and drops the secret after", async () => {
    const { io, credentials, sql } = fakeIO(async (text) =>
      text.includes("users")
        ? { id: ARROW.int64 }
        : { total: ARROW.float64 },
    );

    const { descriptors } = await profile([users, orders], io);

    expect(credentials).toHaveBeenCalledTimes(1);
    expect(credentials).toHaveBeenCalledWith({ connection: "w" }, "read", { signal: expect.any(AbortSignal) });
    expect(sql[0]).toMatch(/^CREATE OR REPLACE SECRET fossil_read_[0-9a-f]{16} \(TYPE s3, /);
    expect(describes(sql)).toEqual([
      `SELECT * FROM read_csv_auto('${W}users.csv') LIMIT 0`,
      `SELECT * FROM read_csv_auto('${W}orders.csv') LIMIT 0`,
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
    const { io, credentials, sql } = fakeIO(async () => ({ id: ARROW.int32 }));
    const { descriptors } = await profile(
      [{ role: "data", binding: "p", key: "https://x.test/p.csv", location: "https://x.test/p.csv", format: "csv" }],
      io,
    );
    expect(credentials).not.toHaveBeenCalled();
    expect(sql).toEqual(["SELECT * FROM read_csv_auto('https://x.test/p.csv') LIMIT 0"]);
    expect(descriptors).toHaveLength(1);
  });

  it("describes a parquet source as parquet, and carries a csv source's delimiter", async () => {
    const { io, sql } = fakeIO(async () => ({ ts: ARROW.timestamp }));
    await profile(
      [
        { role: "data", binding: "e", key: "@w/e.parquet", location: `${W}e.parquet`, connection: "w", format: "parquet" },
        { ...users, option: "|" },
      ],
      io,
    );
    expect(describes(sql)).toEqual([
      `SELECT * FROM read_parquet('${W}e.parquet') LIMIT 0`,
      `SELECT * FROM read_csv_auto('${W}users.csv', delim='|') LIMIT 0`,
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

  it("runs the whole plan when no depth is asked for: the counts after the columns", async () => {
    const { io, sql } = fakeIO(async () => ({ id: ARROW.int64 }), { options: undefined });
    const { profile: profiled } = await profile([users], io);
    expect(sql.some((s) => s.startsWith("SELECT CAST(count(*) AS VARCHAR)"))).toBe(true);
    expect(profiled.sources.map((s) => [s.binding, s.descriptor.key])).toEqual([["users", "@w/users.csv"]]);
  });

  it("asks the host for an etag per source", async () => {
    const etag = vi.fn((source: Input) => `etag-for-${source.key}`);
    const { io } = fakeIO(async () => ({ id: ARROW.int32 }), { etag });
    const { descriptors } = await profile([users], io);
    expect(etag).toHaveBeenCalledWith(users);
    expect(descriptors[0]?.etag).toBe("etag-for-@w/users.csv");
  });

  it("is best-effort: an unvended, unaddressable or unreadable source is answered with its problem and skipped", async () => {
    const { io, sql } = fakeIO(
      async (text) => {
        if (text.includes("orders")) throw new Error("CORS / unreachable");
        return { total: ARROW.int32 };
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

  it("stops a query of the columns that hangs when the caller does, and still gives the credential back", async () => {
    const stop = new AbortController();
    const { io, sql } = fakeIO(() => new Promise(() => {}), { signal: stop.signal });
    io.engine = {
      ...io.engine,
      query: (text, options) =>
        asksColumns(text)
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
