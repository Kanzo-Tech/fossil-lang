/**
 * What the sources a program reads are, read on the host's DuckDB.
 *
 * Which sources a program reads is fossil's answer (`FossilProgram.inputs`), with every `@conn/path`
 * expanded into a location and the connection it goes through. Each one is read under the
 * credential the host vends for that connection (`@fossil-lang/storage`), and `fossil-profile`'s
 * plan runs on the host's engine ({@link runProfiler}): the columns and their types, and — unless
 * the host asks for types alone — the counts, candidate keys, inclusions between sources and a
 * sample. Every source in the answer is named by its binding, the name the program writes. The plan is Rust, the same one `fossil-exec`'s tests run on native DuckDB, so only the
 * engine differs. The engine is the host's, which is why DuckDB-WASM is not a peer.
 *
 * {@link profileInputs} is it over inputs: `FossilProgram.describe` passes the program's own, and
 * `profile` in `./program.ts` the ones the sources a host names are, before there is a program.
 */

import { mount, type Mount } from "@fossil-lang/storage";
import {
  FossilError,
  isFossilError,
  type DataType,
  type Engine,
  type Field,
  type Host,
  type Input,
  type Profile,
  type Profiled,
  type ProfileAnswer,
  type ProfileOptions,
  type ProfileSource,
  type UndescribedSource,
} from "@fossil-lang/types";
import { attachCause } from "@fossil-lang/types/internal";

import { NATIVE_ROWS, type NativeRow } from "./catalogue.generated.js";
import { runProfiler } from "./profile.js";

/** A data input a host can DESCRIBE — the rows `catalogue.bnf` gives a `reads native <fn>`, bound to
 *  a name. A materialised row (`io.rdf`) takes its schema from its shape instead. */
type NativeInput = Input & { format: NativeRow; binding: string };

function isNative(input: Input): input is NativeInput {
  return (
    input.role === "data" &&
    input.binding !== undefined &&
    (NATIVE_ROWS as readonly (string | undefined)[]).includes(input.format)
  );
}

/** What a host gives a profile: its credentials, the page's engine, and how far to go. */
export interface ProfileIO {
  host: Host;
  engine: Engine;
  /**
   * The source's entity tag (RFC 9110 §8.8.3) — an `ETag`, a `Last-Modified`, a version id: an
   * opaque validator, weak or strong. Only the host can produce one cheaply. Absent, descriptors
   * carry `""` and the compiler asks again on every compile.
   */
  etag?(source: Input): Promise<string> | string;
  /**
   * How far the profile goes: `{ depth: "types" }` is one query a source, what an editor
   * needs; absent, the whole profile, with `sample` rows read for the counts and `shown` kept.
   */
  options?: ProfileOptions;
  /** Stops it: the credential requests and every query, rejecting with the signal's reason. */
  signal?: AbortSignal;
}

/** A field as a plain object: an apache-arrow type keeps its `typeId` and parameters on the class,
 *  where the module's reader does not look. */
function fieldOf({ name, nullable, type }: Field): Field {
  return { name, nullable, type: typeOf(type) };
}

function typeOf(t: DataType): DataType {
  const { typeId, bitWidth, isSigned, precision, scale, unit, timezone, byteWidth, listSize, keysSorted, mode } = t;
  return {
    typeId,
    bitWidth,
    isSigned,
    precision,
    scale,
    unit,
    timezone,
    byteWidth,
    listSize,
    keysSorted,
    mode,
    typeIds: t.typeIds === undefined ? undefined : Array.from(t.typeIds),
    children: t.children?.map(fieldOf),
    dictionary: t.dictionary === undefined ? undefined : typeOf(t.dictionary),
    indices: t.indices === undefined ? undefined : typeOf(t.indices),
  };
}

/** An engine's answer as the plan reads it: its fields, and every cell its text or `null`. */
function answerOf(table: Awaited<ReturnType<Engine["query"]>>): ProfileAnswer {
  const columns = table.schema.fields.map((f) => table.getChild(f.name));
  return {
    fields: table.schema.fields.map(fieldOf),
    rows: Array.from({ length: table.numRows }, (_, i) =>
      columns.map((column) => {
        const value: unknown = column?.get(i);
        return value === null || value === undefined ? null : String(value);
      }),
    ),
  };
}

/**
 * Profile every native data input. The host is asked once per connection named, and the credential
 * it vends is given back when this returns.
 *
 * Best-effort: a source the host vends nothing for, or one DuckDB cannot describe, is skipped, never
 * thrown, and answered in `undescribed` with its problem — the editor degrades to no field
 * completion for that source. A source with no connection is read as it is only when it is a public
 * `http(s)` URL.
 *
 * @throws {FossilError} only what giving the credentials back raised; an abort rejects with the
 *   signal's reason.
 */
export async function profileInputs(inputs: readonly Input[], io: ProfileIO): Promise<Profiled> {
  const { signal } = io;
  const native = inputs.filter(isNative);
  const undescribed: UndescribedSource[] = [];
  const empty: Profile = { sources: [], inclusions: [], failed: [] };
  if (native.length === 0) return { profile: empty, descriptors: [], undescribed };

  const mounts = new Map<string, Promise<Mount>>();
  const mountOf = (connection: string): Promise<Mount> => {
    let pending = mounts.get(connection);
    if (pending === undefined) {
      pending = mount(io.engine, io.host, { connection }, "read", { signal });
      mounts.set(connection, pending);
    }
    return pending;
  };
  let outcome: Profiled | undefined;
  let failure: unknown;
  try {
    // Each source as the engine reads it: through its connection's mount, or as a public URL.
    const readable = await Promise.all(
      native.map(async (source): Promise<ProfileSource | undefined> => {
        try {
          let name = source.location;
          if (source.connection !== undefined) {
            const mounted = await mountOf(source.connection);
            name = (await mounted.files([source.location]))[0] ?? name;
          } else if (!/^https?:\/\//.test(source.location)) {
            throw FossilError.of("storage/no-route", { location: source.location });
          }
          const etag = (await io.etag?.(source)) ?? "";
          return { binding: source.binding, key: source.key, location: name, format: source.format, option: source.option, etag };
        } catch (err) {
          signal?.throwIfAborted();
          const problem = isFossilError(err) ? err.problem : FossilError.of("engine/failed", {}, { cause: err }).problem;
          undescribed.push({ source, problem });
          return undefined;
        }
      }),
    );
    const sources = readable.filter((s): s is ProfileSource => s !== undefined);
    const profiled = await runProfiler(
      sources,
      async (sql) => answerOf(await io.engine.query(sql, { signal: signal ?? new AbortController().signal })),
      { options: io.options, signal },
    );
    const described = new Set(profiled.sources.map((s) => s.binding));
    for (const { source: binding, reason } of profiled.failed) {
      const source = native.find((s) => s.binding === binding);
      if (source === undefined || described.has(binding) || undescribed.some((u) => u.source === source)) continue;
      undescribed.push({ source, problem: FossilError.of("engine/failed", {}, { cause: new Error(reason) }).problem });
    }
    undescribed.sort((a, b) => inputs.indexOf(a.source) - inputs.indexOf(b.source));
    outcome = { profile: profiled, descriptors: profiled.sources.map((s) => s.descriptor), undescribed };
  } catch (cause) {
    failure = cause;
  }
  const closed = await Promise.allSettled(
    // eslint-disable-next-line no-restricted-syntax -- a mount that failed is already a source's problem above, so there is nothing of it to close
    [...mounts.values()].map(async (pending) => (await pending.catch(() => undefined))?.close()),
  );
  const cleanup = closed.filter((c) => c.status === "rejected").map((c) => c.reason as unknown);
  if (failure !== undefined) {
    for (const c of cleanup) attachCause(failure, c);
    throw failure;
  }
  if (cleanup.length > 0) throw cleanup[0];
  return outcome!;
}
