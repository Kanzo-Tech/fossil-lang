/**
 * A Mosaic `Coordinator` over the DuckDB this app already booted — **and never a second one.**
 *
 * ## The decision this file reverses, and the two conditions it had to meet
 *
 * `/docs/design/discarded` records a crossfilter in the playground as rejected, and records what
 * would bring it back: «a chart beside the canvas, and the two DuckDB pins agreeing — either an
 * override that the API is measured to survive, or the playground moving to the version the
 * bindings need». Both are met, and this file is the second one.
 *
 * `@uwdata/mosaic-core@0.29.2` depends on `@duckdb/duckdb-wasm@1.33.1-dev57.0` — an exact pin, not
 * a range — and this app pins `1.32.0`. Left alone that is two engines in one tab, which is two
 * crossfilters that never hear each other and a second multi-megabyte wasm asset to boot them
 * against. The root `package.json` forces mosaic-core's copy down to 1.32.0, and the reason that
 * is safe is measured rather than hoped:
 *
 * 1. `wasmConnector` takes an optional pre-existing `duckdb` and `connection`
 *    (`@uwdata/mosaic-core/src/connectors/wasm.ts`). Given both, its `connect()` is never reached,
 *    and `connect()` is the only path to `initDatabase()` — the one function that fetches a
 *    jsDelivr bundle and spawns a worker. **We give it both**, so no bundle is selected, nothing is
 *    fetched, and no second worker exists.
 * 2. The whole surface it then uses on the injected objects is one call:
 *    `con.useUnsafe((bindings, conn) => bindings.runQuery(conn, sql))`. And
 *    `dist/types/src/parallel/async_connection.d.ts` is **byte-identical** between 1.32.0 and
 *    1.33.1-dev57.0 — same 3,291 bytes, md5 `d55a6a4f3348f5d874a427117246361f`. That file is the
 *    contract the override rests on; check its md5 before bumping either pin.
 *
 * So the engine here is `src/duckdb.ts`'s, the same one the corpus is read through, the same one
 * introspection runs on. The crossfilter and the canvas are looking at one database because they
 * are looking at one connection.
 *
 * ## Why the connection is shared rather than a second one off the same instance
 *
 * `instance().connect()` would be legal and cheap, and it is still wrong: DuckDB-WASM registers
 * files on the *database*, but a second connection is a second transaction context and a second
 * place for the app's `SET`s to not be. The app has one connection by design — `src/duckdb.ts`
 * says so — and a crossfilter is not a reason to grow a second.
 *
 * `useUnsafe` serialises onto that connection, which means a coordinator query and a canvas query
 * queue behind each other rather than racing. That is a real cost and it is the right one: they
 * are reading the same tiles, and two connections would have them competing for the same buffer
 * manager anyway.
 */
import { Coordinator, wasmConnector } from '@kanzo-tech/mosaic';

import { connection, instance } from './duckdb.js';

let coordinator: Coordinator | null = null;

/**
 * The one coordinator, built on first use.
 *
 * Lazy rather than module-scope, because `boot()` has to have run: `instance()` and `connection()`
 * throw before it, and a module-level call would make importing this file an ordering constraint
 * on every module that touches it. Every caller is downstream of the corpus being open, so by the
 * time anything asks there is an engine to hand over.
 *
 * Idempotent, and that is the whole safety property: two `Coordinator`s over one connection would
 * be two crossfilters again — the same failure the version override exists to prevent, arrived at
 * from the other side.
 */
export function coordinatorFor(): Coordinator {
  if (coordinator === null) {
    coordinator = new Coordinator();
    // Both objects, which is what keeps `initDatabase()` unreachable. Passing only `duckdb` would
    // let it open a connection of its own; passing neither would let it fetch a bundle from
    // jsDelivr, which this app's whole claim forbids twice over — a second engine and a network
    // request on boot.
    coordinator.databaseConnector(wasmConnector({ duckdb: instance(), connection: connection() }));
  }
  return coordinator;
}

/**
 * Forget the coordinator, so the next caller builds one against whatever engine is current.
 *
 * Not a teardown of the engine — that belongs to `src/duckdb.ts` and nothing here owns it. This is
 * for the case where the app re-runs a program and the corpus underneath the clients changes: the
 * clients are disconnected by their own owners, and this drops the cached coordinator so a stale
 * one cannot outlive them.
 */
export function resetCoordinator(): void {
  coordinator?.clear();
  coordinator = null;
}
