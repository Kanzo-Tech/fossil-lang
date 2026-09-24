/**
 * The parity guard.
 *
 * `packages/introspect` and `crates/fossil-introspect` implement one capability
 * twice — the browser's DESCRIBE through DuckDB-WASM and the native host's
 * through `duckdb` — and they must agree or the browser and the CLI answer the
 * same program differently. Which sources a program reads is not one of the
 * things they share: the browser takes them from fossil's AST
 * (`FossilPlayground.sources`), so the regex that used to be compared here is
 * gone from this side, and this file builds its `ProgramSource`s by hand.
 *
 * What still has to agree, and how each is checked:
 *
 * - **The readers** are two GENERATED projections of `catalogue.bnf`, by two
 *   different emitters. `cargo xtask catalogue --check` proves each file
 *   matches its own emitter and nothing compares the two, so this does: it
 *   reads `providers/generated.rs` and asserts `introspect` DESCRIBEs through
 *   what the Rust names, for every row, with neither side holding a row the
 *   other lacks.
 * - **The option keyword** — what DuckDB calls a row's reader option — is
 *   written by hand on both sides, and this reads `duckdb_option_keyword`.
 * - **The type table** is written by hand on both sides, and this reads
 *   `duckdb_type_to_fossil_primitive`.
 *
 * The DESCRIBE statement itself is still composed twice and is not compared;
 * making it one is a separate step.
 *
 * Nothing here restates a value as a literal — a copy of the answer cannot
 * notice the answer changing. Every extraction throws when the shape it expects
 * is gone: a guard that silently finds nothing and passes is the failure this
 * file exists to avoid, so "the Rust moved" fails here rather than going quiet.
 *
 * WHAT IT CANNOT PROVE: that either side is right (only that they say the same
 * thing); that the Rust behaves as its source reads (this does not run
 * `fossil-introspect`); that the generated files are current
 * (`crates/xtask/tests/catalogue_generated.rs` owns that).
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ProgramSource } from "@fossil-lang/types";
import { describe, expect, it } from "vitest";
import {
  duckdbTypeToFossilPrimitive,
  introspect,
  type InferredPrimitive,
  type SourceFormat,
} from "../src/index.js";
import { NATIVE_ROWS } from "../src/catalogue.generated.js";

const ENGINE_REL = "crates/fossil-introspect/src/lib.rs";

/** The repo root, found by walking up until the engine crate is under it. */
function engineSource(): string {
  let dir = dirname(fileURLToPath(import.meta.url));
  for (let i = 0; i < 8; i++) {
    try {
      return readFileSync(join(dir, ENGINE_REL), "utf8");
    } catch {
      dir = dirname(dir);
    }
  }
  throw new Error(
    `parity guard cannot find ${ENGINE_REL} above ${fileURLToPath(import.meta.url)}; ` +
      "if the crate moved, repoint this guard — do not delete it",
  );
}

const RUST = engineSource();

function must(re: RegExp, what: string): RegExpMatchArray {
  const m = RUST.match(re);
  if (!m) {
    throw new Error(
      `parity guard cannot find ${what} in ${ENGINE_REL}; the Rust was rewritten ` +
        "and this guard must be rewritten with it",
    );
  }
  return m;
}

/** The generated Rust catalogue, the counterpart of `catalogue.generated.ts`. */
const GENERATED_REL = "crates/fossil-base/src/providers/generated.rs";

/**
 * `row name → DuckDB table function`, read out of the GENERATED Rust.
 *
 * This is not the same guard as before and it is worth saying why. The reader
 * dispatch used to be a hand-written `match` in `fossil-engine`, so the test
 * compared two hand-written tables. Both sides are generated from
 * `catalogue.bnf` now — but by two DIFFERENT emitters in `xtask`, and
 * `cargo xtask catalogue --check` only proves each file matches its own
 * emitter. Nothing else compares the two projections, and an emitter that
 * derived `CsvAuto` from `read_csv_auto` one way in Rust and another way in
 * TypeScript would pass every other check in the tree.
 */
function rustNativeReaders(): Map<string, { variant: string; fn: string }> {
  let dir = dirname(fileURLToPath(import.meta.url));
  let generated = "";
  for (let i = 0; i < 8; i++) {
    try {
      generated = readFileSync(join(dir, GENERATED_REL), "utf8");
      break;
    } catch {
      dir = dirname(dir);
    }
  }
  if (generated === "") {
    throw new Error(
      `parity guard cannot find ${GENERATED_REL}; run \`cargo xtask catalogue\``,
    );
  }

  // variant → table function, from `NativeReader::table_function`.
  const fns = new Map<string, string>();
  for (const m of generated.matchAll(/Self::(\w+) => "([^"]+)",/g)) {
    fns.set(m[1]!, m[2]!);
  }
  // row name → variant, from each `pub static` block.
  const out = new Map<string, { variant: string; fn: string }>();
  for (const m of generated.matchAll(
    /pub static \w+: Provider = Provider \{\s*name: "([^"]+)",[\s\S]*?reads_rows: ([^\n]+),/g,
  )) {
    const native = m[2]!.match(/RowReader::Native\(NativeReader::(\w+)\)/);
    if (!native) continue;
    const fn = fns.get(native[1]!);
    if (!fn) {
      throw new Error(
        `parity guard: \`NativeReader::${native[1]}\` has no \`table_function\` arm`,
      );
    }
    out.set(m[1]!, { variant: native[1]!, fn });
  }
  if (out.size === 0) {
    throw new Error(
      "parity guard read no native rows out of the generated Rust; it changed shape",
    );
  }
  return out;
}

/** `NativeReader` variant → the DuckDB option keyword, or `null` for none. */
function rustOptionKeywords(): Map<string, string | null> {
  const block = must(
    /fn duckdb_option_keyword[\s\S]*?match r \{([\s\S]*?)\n    \}/,
    "`duckdb_option_keyword`'s match",
  )[1]!;
  const out = new Map<string, string | null>();
  for (const line of block.split("\n")) {
    const t = line.trim();
    if (t === "" || t.startsWith("//")) continue;
    const arm = t.match(/^((?:fossil_base::NativeReader::\w+\s*\|\s*)*fossil_base::NativeReader::\w+) => (Some\("([^"]+)"\)|None),$/);
    if (!arm) throw new Error(`parity guard cannot read option arm \`${t}\``);
    for (const v of arm[1]!.split("|")) {
      out.set(v.trim().replace("fossil_base::NativeReader::", ""), arm[3] ?? null);
    }
  }
  if (out.size === 0) {
    throw new Error("parity guard read no option arms; the match changed shape");
  }
  return out;
}

/** `Primitive::DateTime` → `"date_time"`, the wire spelling. */
function wireName(variant: string): InferredPrimitive {
  return variant
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .toLowerCase() as InferredPrimitive;
}

/**
 * The DuckDB type table: exact spellings, the `starts_with` guard arm, and the
 * wildcard, each kept apart because they are checked differently.
 */
function rustTypeTable(): {
  exact: Map<string, InferredPrimitive>;
  prefixes: Array<[string, InferredPrimitive]>;
  fallback: InferredPrimitive;
} {
  const block = must(
    /fn duckdb_type_to_fossil_primitive[\s\S]*?match upper\.as_str\(\) \{([\s\S]*?)\n    \}/,
    "`duckdb_type_to_fossil_primitive`'s match",
  )[1]!;
  const exact = new Map<string, InferredPrimitive>();
  const prefixes: Array<[string, InferredPrimitive]> = [];
  let fallback: InferredPrimitive | undefined;
  for (const line of block.split("\n")) {
    const t = line.trim();
    if (t === "" || t.startsWith("//")) continue;
    const literals = t.match(/^((?:"[^"]+"\s*\|\s*)*"[^"]+") => Primitive::(\w+),$/);
    if (literals) {
      for (const lit of literals[1]!.split("|")) {
        exact.set(lit.trim().slice(1, -1), wireName(literals[2]!));
      }
      continue;
    }
    const prefix = t.match(
      /^t if t\.starts_with\("([^"]+)"\) => Primitive::(\w+),$/,
    );
    if (prefix) {
      prefixes.push([prefix[1]!, wireName(prefix[2]!)]);
      continue;
    }
    const wild = t.match(/^_ => Primitive::(\w+),$/);
    if (wild) {
      fallback = wireName(wild[1]!);
      continue;
    }
    throw new Error(`parity guard cannot read type arm \`${t}\``);
  }
  if (exact.size === 0 || prefixes.length === 0 || fallback === undefined) {
    throw new Error("parity guard read an incomplete type table; the match changed shape");
  }
  return { exact, prefixes, fallback };
}

/** The constructors this package knows, checked for plausibility first. */
function mustFormats(): SourceFormat[] {
  const formats = [...NATIVE_ROWS] as SourceFormat[];
  // A generated module that came out empty would give every loop below a
  // corpus of nothing and pass by asserting nothing — the failure this whole
  // file exists to avoid.
  if (formats.length === 0 || !formats.every((f) => /^[a-z][a-z0-9_]*$/.test(f))) {
    throw new Error(
      `parity guard read an implausible constructor list: ${JSON.stringify(formats)}`,
    );
  }
  return formats;
}

/** One source per native row, as fossil would report it, with `option` on each. */
function sourcesFor(formats: readonly SourceFormat[], option?: string): ProgramSource[] {
  return formats.map((format, i) => ({
    binding: `s${i}`,
    key: `data/${i}.${format}`,
    locator: `s3://bucket/data/${i}.${format}`,
    format,
    option,
  }));
}

/** The DESCRIBE `introspect` sends for each source, in order. */
async function describesOf(sources: ProgramSource[]): Promise<string[]> {
  const seen: string[] = [];
  await introspect(sources, {
    host: {
      connections: async () => ({}),
      sign: async (locators) => Object.fromEntries(locators.map((l) => [l, l])),
    },
    register: async () => {},
    query: async (sql) => {
      seen.push(sql);
      return [];
    },
    onWarn: (message, err) => {
      throw new Error(`${message}: ${String(err)}`);
    },
  });
  return seen;
}

describe("reader dispatch parity with the generated Rust catalogue", () => {
  it("describes every constructor through the reader the generated Rust names", async () => {
    const readers = rustNativeReaders();
    const formats = mustFormats();
    const sources = sourcesFor(formats);
    expect(await describesOf(sources)).toEqual(
      sources.map((s) => {
        const reader = readers.get(s.format);
        expect(reader, `no native reader for \`io.${s.format}\` in the Rust`).toBeDefined();
        return `DESCRIBE SELECT * FROM ${reader!.fn}('${s.key}')`;
      }),
    );
  });

  it("names the same native rows on both sides — neither has one the other lacks", () => {
    expect([...rustNativeReaders().keys()].sort()).toEqual(
      [...mustFormats()].sort(),
    );
  });
});

describe("reader option parity with fossil-introspect", () => {
  it("sends a row's option under the keyword the Rust names, and drops it where the Rust has none", async () => {
    const readers = rustNativeReaders();
    const keywords = rustOptionKeywords();
    const sources = sourcesFor(mustFormats(), "|");
    expect(await describesOf(sources)).toEqual(
      sources.map((s) => {
        const { variant, fn } = readers.get(s.format)!;
        expect(keywords.has(variant), `no option arm for \`${variant}\``).toBe(true);
        const keyword = keywords.get(variant);
        const args = keyword ? `, ${keyword}='|'` : "";
        return `DESCRIBE SELECT * FROM ${fn}('${s.key}'${args})`;
      }),
    );
  });
});

describe("DuckDB type table parity with fossil-introspect", () => {
  it("maps every spelling the Rust names to the same primitive", () => {
    const { exact } = rustTypeTable();
    for (const [spelling, primitive] of exact) {
      expect([spelling, duckdbTypeToFossilPrimitive(spelling)]).toEqual([
        spelling,
        primitive,
      ]);
    }
  });

  it("honours the prefix arms and the wildcard", () => {
    const { prefixes, fallback } = rustTypeTable();
    for (const [prefix, primitive] of prefixes) {
      expect(duckdbTypeToFossilPrimitive(`${prefix}(10,2)`)).toBe(primitive);
    }
    expect(duckdbTypeToFossilPrimitive("STRUCT(a INT)")).toBe(fallback);
  });
});
