/**
 * @fossil-lang/introspect — source-binding schema introspection (the one home).
 *
 * "Given the sources a program reads, produce an `InferredDescriptor` per
 * source" is a single fossil capability. Which sources a program reads is not
 * this package's question: fossil answers it from the AST
 * (`FossilProgram.inputs`), with every `@conn/path` already expanded into
 * a location and the connection it goes through. This package reads each one
 * through the host's DuckDB under the credential the host vends for that
 * connection (`@fossil-lang/storage`), and owns the DESCRIBE SQL, the
 * DuckDB→primitive table and the descriptor shape. The engine is the host's,
 * which is why DuckDB-WASM is not a peer.
 *
 * The descriptor and its `Primitive` are `@fossil-lang/types`', generated from
 * the Rust that reads them.
 *
 * `fossil-introspect` does the same job natively. The constructors that exist,
 * the reader each picks, its option keyword and the DuckDB→primitive table are
 * generated from `catalogue.bnf` into `catalogue.generated.ts`, and into the
 * Rust from the same file.
 */

import { mount, type Mount } from "@fossil-lang/storage";
import {
  FossilError,
  isFossilError,
  type Engine,
  type Host,
  type InferredDescriptor,
  type Input,
  type Problem,
} from "@fossil-lang/types";
import { attachCause } from "@fossil-lang/types/internal";

import { NATIVE_ROWS, type NativeRow } from "./catalogue.generated.js";
import { buildDescriptor, describeSql, type DescribeRow } from "./describe.js";

/** A data input whose constructor this package can DESCRIBE — the rows `catalogue.bnf` gives a
 *  `reads native <fn>`. A materialised row (`io.rdf`) takes its schema from its shape instead. */
type NativeInput = Input & { format: NativeRow };

function isNative(input: Input): input is NativeInput {
  return input.role === "data" && (NATIVE_ROWS as readonly (string | undefined)[]).includes(input.format);
}

/** What a host gives introspection: its credentials and the page's engine. */
export interface IntrospectOptions {
  host: Host;
  engine: Engine;
  /**
   * The source's entity tag (RFC 9110 §8.8.3) — an `ETag`, a `Last-Modified`, a version id: an
   * opaque validator, weak or strong. Only the host can produce one cheaply. Absent, descriptors
   * carry `""` and the compiler re-introspects on every compile.
   */
  etag?(source: Input): Promise<string> | string;
  /** Stops it: the credential requests and every DESCRIBE, rejecting with the signal's reason. */
  signal?: AbortSignal;
}

/** A source introspection could not describe, and why. */
export interface UndescribedSource {
  source: Input;
  problem: Problem;
}

/**
 * Describe every native data input and return the descriptors, in `inputs` order. The host is asked
 * once per connection named, and the credential it vends is given back when this returns.
 *
 * Best-effort: a source the host vends nothing for, or one DuckDB cannot read, is skipped, never
 * thrown, and answered in `undescribed` with its problem — the editor degrades to no field
 * completion for that source. A source with no connection is read as it is only when it is a public
 * `http(s)` URL. The host hands the whole result to `FossilProgram.registerIntrospection`: the
 * descriptors type the program, and each undescribed source is a warning of `diagnostics` at its call.
 *
 * @throws {FossilError} only what giving the credentials back raised; an abort rejects with the
 *   signal's reason.
 */
export async function introspect(
  inputs: readonly Input[],
  io: IntrospectOptions,
): Promise<{ descriptors: InferredDescriptor[]; undescribed: UndescribedSource[] }> {
  const { signal } = io;
  const native = inputs.filter(isNative);
  const undescribed: UndescribedSource[] = [];
  if (native.length === 0) return { descriptors: [], undescribed };

  const mounts = new Map<string, Promise<Mount>>();
  const mountOf = (connection: string): Promise<Mount> => {
    let pending = mounts.get(connection);
    if (pending === undefined) {
      pending = mount(io.engine, io.host, { connection }, "read", { signal });
      mounts.set(connection, pending);
    }
    return pending;
  };
  let outcome: InferredDescriptor[] | undefined;
  let failure: unknown;
  try {
    const described = await Promise.all(
      native.map(async (source) => {
        try {
          let name = source.location;
          if (source.connection !== undefined) {
            const mounted = await mountOf(source.connection);
            name = (await mounted.files([source.location]))[0] ?? name;
          } else if (!/^https?:\/\//.test(source.location)) {
            throw FossilError.of(
              "storage/no-route",
              { location: source.location },
            );
          }
          const described = await io.engine.query(describeSql(name, source.format, source.option), {
            signal: signal ?? new AbortController().signal,
          });
          const names = described.getChild("column_name");
          const types = described.getChild("column_type");
          const rows: DescribeRow[] = Array.from({ length: described.numRows }, (_, i) => ({
            column_name: names?.get(i),
            column_type: types?.get(i),
          }));
          return buildDescriptor(source.key, rows, source.format, await io.etag?.(source));
        } catch (err) {
          signal?.throwIfAborted();
          const problem = isFossilError(err)
            ? err.problem
            : FossilError.of("engine/failed", {}, {
                cause: err,
              }).problem;
          undescribed.push({ source, problem });
          return undefined;
        }
      }),
    );
    outcome = described.filter((d): d is InferredDescriptor => d !== undefined);
    undescribed.sort((a, b) => inputs.indexOf(a.source) - inputs.indexOf(b.source));
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
  return { descriptors: outcome!, undescribed };
}
