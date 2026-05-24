/**
 * Token kind → CodeMirror highlight-tag mapping for the Fossil language
 * extension.
 *
 * `FossilKind` mirrors the variant ORDER of `fossil_syntax::lexer::Token` in
 * `crates/fossil-syntax/src/lexer.rs` (lines 32-227). Each variant's numeric
 * value is the u32 discriminant logos assigns by declaration order — the
 * Rust side passes that same u32 through `wasm-bindgen` as `TokenRow.kind`
 * (see `@fossil-lang/types::TokenRow`).
 *
 * Per ADR-0030 § Negative consequences: REORDERING the Rust `Token` enum is
 * a BREAKING CHANGE for `@fossil-lang/codemirror-fossil` — bump major in
 * lockstep. APPENDING new variants is backwards-compatible: any kind not in
 * `KIND_TO_TAG` falls back to `null` (CodeMirror's default text style).
 *
 * Mapping philosophy (v0.1):
 *   - Keywords (`prefix`, `from`, `in`, `use`, `as`, `and`, `or`, `not`,
 *     `iri`) → `'keyword'`
 *   - Attribute markers (`@export`, `@dcat`, …) → `'meta'` (semantic
 *     decoration, not a value)
 *   - Numeric / string / template literals → `'number'` / `'string'`
 *   - Absolute IRIs (`<https://…>`) → `'string'` (they are URI literals)
 *   - `$ENV_VAR` references → `'variableName'`
 *   - Operators (`:=`, `==`, `|>`, arithmetic, comparison) → `'operator'`
 *   - Punctuation (parens / braces / dot / comma / colon) → `'punctuation'`
 *   - Identifiers → `null` (no opinionated highlight; the LSP semantic-tokens
 *     overlay in 08-09 paints these as `variable`/`function`/`type` etc.)
 *   - `Partial` (`_`) → `'keyword'` (it's a language-level placeholder)
 *   - Trivia (`Whitespace` / `Newline` / `Comment`) — `Comment` → `'comment'`;
 *     the indent pass also keeps `Whitespace` and `Newline` tokens, but those
 *     are stripped by the lexer before `tokenize()` returns (the raw_lex
 *     output we receive includes them — they get mapped to `null` so
 *     CodeMirror renders them as plain text).
 */

/**
 * Numeric discriminants of `fossil_syntax::lexer::Token`, in declaration order
 * (logos assigns 0, 1, 2, … by source position). MUST stay in sync with
 * `crates/fossil-syntax/src/lexer.rs` — reorders are a breaking change.
 */
export enum FossilKind {
  // Trivia
  Whitespace = 0,
  Newline = 1,
  Comment = 2,
  // Keywords
  KwPrefix = 3,
  KwFrom = 4,
  KwIn = 5,
  KwUse = 6,
  KwAs = 7,
  KwAnd = 8,
  KwOr = 9,
  KwNot = 10,
  KwIri = 11,
  // Attribute markers
  AtExport = 12,
  AtAttr = 13,
  // Numeric literals
  Float = 14,
  Integer = 15,
  // Identifier / partial-application placeholder
  Ident = 16,
  Partial = 17,
  // String / template / IRI / env-var literals
  String = 18,
  Template = 19,
  AbsIri = 20,
  EnvVar = 21,
  // Multi-character operators (longer-first per logos longest-match)
  Define = 22,
  TypeAnnot = 23,
  Eq = 24,
  Neq = 25,
  Le = 26,
  Ge = 27,
  Arrow = 28,
  Pipe = 29,
  TripleOpen = 30,
  TripleClose = 31,
  // Single-character operators / punctuation
  Assign = 32,
  Colon = 33,
  Dot = 34,
  Comma = 35,
  LParen = 36,
  RParen = 37,
  LBrace = 38,
  RBrace = 39,
  Lt = 40,
  Gt = 41,
  Plus = 42,
  Minus = 43,
  Star = 44,
  Slash = 45,
  Percent = 46,
  Question = 47,
  ShapeAnd = 48,
}

/**
 * Highlight-scope name returned to CodeMirror's `StreamParser.token()` callback.
 *
 * The names are the canonical `@lezer/highlight` tag names — CodeMirror's
 * `defaultHighlightStyle` maps these to its built-in theme, and consumer themes
 * key on the same names. Using strings (rather than `Tag` objects) keeps the
 * StreamParser callback path zero-allocation per token and matches the
 * `StreamParser<State>.token()` return-type contract (`string | null`).
 */
export const KIND_TO_TAG: Record<number, string | null> = {
  // Trivia — Whitespace + Newline are usually stripped before they reach the
  // StreamParser path, but we map them defensively so an upstream change
  // doesn't crash the highlighter.
  [FossilKind.Whitespace]: null,
  [FossilKind.Newline]: null,
  [FossilKind.Comment]: 'comment',

  // Keywords — single category, matches CodeMirror's built-in `keyword` tag.
  [FossilKind.KwPrefix]: 'keyword',
  [FossilKind.KwFrom]: 'keyword',
  [FossilKind.KwIn]: 'keyword',
  [FossilKind.KwUse]: 'keyword',
  [FossilKind.KwAs]: 'keyword',
  [FossilKind.KwAnd]: 'keyword',
  [FossilKind.KwOr]: 'keyword',
  [FossilKind.KwNot]: 'keyword',
  [FossilKind.KwIri]: 'keyword',

  // Attribute markers — semantic decoration, not value. `meta` is the
  // canonical Lezer tag for "language-machinery markers" (preprocessor
  // directives, attributes, derive macros etc.).
  [FossilKind.AtExport]: 'meta',
  [FossilKind.AtAttr]: 'meta',

  // Numeric literals
  [FossilKind.Float]: 'number',
  [FossilKind.Integer]: 'number',

  // Identifier — DELIBERATELY null at the syntactic layer. The LSP
  // semantic-tokens overlay (08-09) paints these as `variable`/`function`/
  // `type`/`property` based on HIR resolution.
  [FossilKind.Ident]: null,

  // Partial-application placeholder `_` — it's a language-level token, not
  // an identifier. `operator` is the closest match in Lezer's vocabulary.
  [FossilKind.Partial]: 'operator',

  // String / template / IRI literals — all string-shaped values.
  [FossilKind.String]: 'string',
  [FossilKind.Template]: 'string',
  [FossilKind.AbsIri]: 'string',

  // $ENV_VAR — a variable reference, but distinct enough to use a stable
  // tag that themes can target separately if they wish. `variableName` is
  // the @lezer/highlight built-in for "variable in scope".
  [FossilKind.EnvVar]: 'variableName',

  // Operators (assignment, comparison, arrow, pipe, triple-quoting, etc.)
  [FossilKind.Define]: 'operator',
  [FossilKind.TypeAnnot]: 'operator',
  [FossilKind.Eq]: 'operator',
  [FossilKind.Neq]: 'operator',
  [FossilKind.Le]: 'operator',
  [FossilKind.Ge]: 'operator',
  [FossilKind.Arrow]: 'operator',
  [FossilKind.Pipe]: 'operator',
  [FossilKind.TripleOpen]: 'operator',
  [FossilKind.TripleClose]: 'operator',
  [FossilKind.Assign]: 'operator',
  [FossilKind.Lt]: 'operator',
  [FossilKind.Gt]: 'operator',
  [FossilKind.Plus]: 'operator',
  [FossilKind.Minus]: 'operator',
  [FossilKind.Star]: 'operator',
  [FossilKind.Slash]: 'operator',
  [FossilKind.Percent]: 'operator',
  [FossilKind.Question]: 'operator',
  [FossilKind.ShapeAnd]: 'operator',

  // Punctuation
  [FossilKind.Colon]: 'punctuation',
  [FossilKind.Dot]: 'punctuation',
  [FossilKind.Comma]: 'punctuation',
  [FossilKind.LParen]: 'punctuation',
  [FossilKind.RParen]: 'punctuation',
  [FossilKind.LBrace]: 'punctuation',
  [FossilKind.RBrace]: 'punctuation',
};

/**
 * Look up the highlight-scope name for a Token kind. Returns `null` for
 * trivia + identifiers (default editor styling) or for unknown kinds (a
 * newly-appended Rust `Token` variant; CodeMirror falls back to the default
 * style — no crash).
 */
export function kindToTagName(kind: number): string | null {
  // Map lookup; deliberate undefined → null normalisation so callers can
  // pass the value straight to StreamParser's token() return path.
  const tag = KIND_TO_TAG[kind];
  return tag ?? null;
}
