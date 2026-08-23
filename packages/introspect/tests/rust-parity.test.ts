/**
 * The parity guard.
 *
 * `packages/introspect` and `crates/fossil-introspect` implement one capability
 * twice — the native half was `fossil-engine`'s until introspection became the
 * host's job — and three things had to agree or the browser and the CLI answer the
 * same program differently: the source-binding pattern, the DuckDB reader each
 * constructor picks, and the DuckDB→primitive table. Until this file existed
 * the agreement was asserted in a comment, and the comment was wrong in both
 * directions — the TS pattern had never learned `io.parquet`, and the comment
 * describing the Rust dispatch described a version of it that predated the
 * `read_json_auto` / `read_parquet` arms.
 *
 * **Two of the three stopped being an agreement.** Which constructors exist and
 * which reader each one names now come from `catalogue.bnf` on both sides —
 * `cargo xtask catalogue` writes `providers/generated.rs` and
 * `catalogue.generated.ts` from it, and neither side carries a list. So what
 * this file checks changed shape:
 *
 * - **The pattern** is still two regexes in two dialects, and the SCAFFOLD
 *   around the alternation is still written twice. That comparison stays,
 *   against the Rust source, with the alternation substituted from the
 *   catalogue rather than scraped.
 * - **The readers** are now two GENERATED projections of one file, by two
 *   different emitters. `cargo xtask catalogue --check` proves each file
 *   matches its own emitter and nothing compares the two, so this does: it
 *   reads `providers/generated.rs` and asserts `describeSql` picks what the
 *   Rust names, for every row, with neither side holding a row the other lacks.
 * - **The type table** is unchanged — it is not catalogue data, it is still
 *   written twice by hand, and this still reads the Rust for it.
 *
 * Nothing here restates a value as a literal — a copy of the answer cannot
 * notice the answer changing. Every extraction throws when the shape it expects
 * is gone: a guard that silently finds nothing and passes is the failure this
 * file exists to avoid, so "the Rust moved" fails here rather than going quiet.
 *
 * WHAT IT CANNOT PROVE, beside what it can:
 *
 * - **That either side is right.** It proves they say the same thing. Both
 *   scrapers are wider than the lexer — `\w` admits a leading digit, which
 *   `fossil-syntax`'s `Ident` (`[A-Za-z_][A-Za-z0-9_]*`) does not — and they
 *   are equally wrong together.
 * - **That the Rust behaves as its source reads.** This is a text comparison:
 *   it does not run `fossil-introspect`. A change to how the crate *uses*
 *   the pattern it compiles is invisible here.
 * - **That identical pattern text means identical matches.** Rust's `\w` is
 *   Unicode-aware and JS's is ASCII, so a non-ASCII binding name is scraped
 *   natively and skipped in the browser. Nothing in fossil can name one
 *   today, which is why this is a note and not a failing test.
 * - **That the generated files are current.** `cargo xtask catalogue --check`
 *   and `crates/xtask/tests/catalogue_generated.rs` own that. Two stale files
 *   that are stale in the same way agree here.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  SOURCE_REF_PATTERN,
  describeSql,
  duckdbTypeToFossilPrimitive,
  extractSourceRefs,
  type InferredPrimitive,
  type SourceFormat,
} from "../src/index.js";
import { NATIVE_ROWS } from "../src/catalogue.generated.js";

// It was `crates/fossil-engine/src/lib.rs`. The scrape moved with the code:
// introspection is the HOST's job and lives in `fossil-introspect` now, which
// is what let `fossil-engine` stop linking a database. The pattern and the type
// table did not change — this file compares the same two implementations.
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

/**
 * The pattern `extract_source_refs` compiles, with the alternation filled in.
 *
 * The Rust used to carry the alternation as a literal; it interpolates
 * `{alternation}` from the catalogue now, so this substitutes the same source
 * the TypeScript uses. What the comparison proves is therefore narrower and
 * truer than before: the SCAFFOLD around the alternation, which is genuinely
 * written twice in two regex dialects, still agrees.
 */
function rustPattern(): string {
  const fn = must(
    /fn extract_source_refs[\s\S]*?format!\(\s*r#"([\s\S]*?)"#\s*\)/,
    "`extract_source_refs`'s pattern literal",
  );
  const literal = fn[1]!;
  if (!literal.includes("{alternation}")) {
    throw new Error(
      "parity guard: the Rust pattern no longer interpolates `{alternation}`; " +
        "if it went back to a literal list, this guard must compare it to the catalogue",
    );
  }
  return literal.replace("{alternation}", NATIVE_ROWS.join("|"));
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
function rustNativeReaders(): Map<string, string> {
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
  const out = new Map<string, string>();
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
    out.set(m[1]!, fn);
  }
  if (out.size === 0) {
    throw new Error(
      "parity guard read no native rows out of the generated Rust; it changed shape",
    );
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

describe("source-binding pattern parity with fossil-engine", () => {
  it("is character-for-character the Rust one", () => {
    expect(SOURCE_REF_PATTERN).toBe(rustPattern());
  });

  it("extracts every constructor the Rust pattern admits", () => {
    const formats = mustFormats();
    const text = formats
      .map((f, i) => `s${i} := io.${f}("data/${i}.${f}")`)
      .join("\n");
    expect(extractSourceRefs(text)).toEqual(
      formats.map((f, i) => ({
        sourceName: `s${i}`,
        format: f,
        url: `data/${i}.${f}`,
      })),
    );
  });
});

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

describe("reader dispatch parity with the generated Rust catalogue", () => {
  it("picks the reader the generated Rust names, for every constructor", () => {
    const readers = rustNativeReaders();
    for (const format of mustFormats()) {
      const reader = readers.get(format);
      expect(reader, `no native reader for \`io.${format}\` in the Rust`).toBeDefined();
      expect(describeSql("x", format)).toBe(
        `DESCRIBE SELECT * FROM ${reader}('x')`,
      );
    }
  });

  it("names the same native rows on both sides — neither has one the other lacks", () => {
    expect([...rustNativeReaders().keys()].sort()).toEqual(
      [...mustFormats()].sort(),
    );
  });
});

describe("DuckDB type table parity with fossil-engine", () => {
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
