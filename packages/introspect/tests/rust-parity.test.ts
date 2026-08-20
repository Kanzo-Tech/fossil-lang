/**
 * The parity guard.
 *
 * `packages/introspect` and `crates/fossil-engine` implement one capability
 * twice, and three things must agree or the browser and the CLI answer the
 * same program differently: the source-binding pattern, the DuckDB reader each
 * constructor picks, and the DuckDB→primitive table. Until this file existed
 * the agreement was asserted in a comment, and the comment was wrong in both
 * directions — the TS pattern had never learned `io.parquet`, and the comment
 * describing the Rust dispatch described a version of it that predated the
 * `read_json_auto` / `read_parquet` arms.
 *
 * So: read the Rust, derive the three tables from it, and compare against what
 * this package DOES. Nothing here restates a Rust value as a TS literal — a
 * copy of the answer cannot notice the answer changing. Every extraction is
 * `must*`, which throws when the shape it expects is gone: a guard that
 * silently finds nothing and passes is the failure this file exists to avoid,
 * so "the Rust moved" fails here rather than going quiet.
 *
 * WHAT IT CANNOT PROVE, beside what it can:
 *
 * - **That either side is right.** It proves they say the same thing. Both
 *   scrapers are wider than the lexer — `\w` admits a leading digit, which
 *   `fossil-syntax`'s `Ident` (`[A-Za-z_][A-Za-z0-9_]*`) does not — and they
 *   are equally wrong together.
 * - **That the Rust behaves as its source reads.** This is a text comparison:
 *   it does not run `fossil-engine`. A change to how the crate *uses* the
 *   pattern it compiles is invisible here.
 * - **That identical pattern text means identical matches.** Rust's `\w` is
 *   Unicode-aware and JS's is ASCII, so a non-ASCII binding name is scraped
 *   natively and skipped in the browser. Nothing in fossil can name one
 *   today, which is why this is a note and not a failing test.
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

const ENGINE_REL = "crates/fossil-engine/src/lib.rs";

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

/** The regex literal `extract_source_refs` compiles, verbatim. */
function rustPattern(): string {
  const fn = must(
    /fn extract_source_refs[\s\S]*?regex::Regex::new\(\s*r#"([\s\S]*?)"#\s*\)/,
    "`extract_source_refs`'s regex literal",
  );
  return fn[1]!;
}

/** `constructor → DuckDB table function`, from the DESCRIBE dispatch. */
function rustReaders(): { arms: Map<string, string>; fallback: string } {
  const block = must(
    /let reader = match constructor\.as_str\(\) \{([\s\S]*?)\};/,
    "the `let reader = match constructor` dispatch",
  )[1]!;
  const arms = new Map<string, string>();
  let fallback = "";
  for (const line of block.split("\n")) {
    const t = line.trim();
    if (t === "" || t.startsWith("//")) continue;
    const arm = t.match(/^"([^"]+)" => "([^"]+)",$/);
    if (arm) {
      arms.set(arm[1]!, arm[2]!);
      continue;
    }
    const wild = t.match(/^_ => "([^"]+)",$/);
    if (wild) {
      fallback = wild[1]!;
      continue;
    }
    throw new Error(`parity guard cannot read reader arm \`${t}\``);
  }
  if (arms.size === 0 || fallback === "") {
    throw new Error("parity guard read no reader arms; the dispatch changed shape");
  }
  return { arms, fallback };
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

/** The constructors named in the Rust alternation — the corpus, never a literal. */
function mustFormats(): SourceFormat[] {
  const alt = rustPattern().match(/io\\\.\(([^)]+)\)/);
  if (!alt) {
    throw new Error("parity guard cannot read the constructor alternation");
  }
  const formats = alt[1]!.split("|") as SourceFormat[];
  // `"".split("|")` is `[""]`, not `[]` — an alternation that read as empty
  // would otherwise give every loop below a corpus of one meaningless entry
  // and pass by asserting nothing.
  if (!formats.every((f) => /^[a-z][a-z0-9_]*$/.test(f))) {
    throw new Error(
      `parity guard read an implausible constructor alternation: ${JSON.stringify(formats)}`,
    );
  }
  return formats;
}

describe("reader dispatch parity with fossil-engine", () => {
  it("picks the reader the Rust picks, for every constructor", () => {
    const { arms, fallback } = rustReaders();
    for (const format of mustFormats()) {
      const reader = arms.get(format) ?? fallback;
      expect(describeSql("x", format)).toBe(
        `DESCRIBE SELECT * FROM ${reader}('x')`,
      );
    }
  });

  it("leaves no Rust reader arm unreachable from a scraped constructor", () => {
    const formats = mustFormats();
    for (const constructor of rustReaders().arms.keys()) {
      expect(formats).toContain(constructor);
    }
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
