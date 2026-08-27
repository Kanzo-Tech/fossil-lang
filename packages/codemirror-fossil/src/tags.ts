/**
 * Lexer variant NAME → `@lezer/highlight` tag.
 *
 * ## Why this table is keyed by name and the old one was not
 *
 * There was a `packages/codemirror-fossil/src/tags.ts` before `873cbc0`, and it
 * opened with a hand-copied `enum FossilKind { Whitespace = 0, Newline = 1, … }`
 * under a comment saying it «MUST stay in sync with
 * `crates/fossil-syntax/src/lexer.rs` — reorders are a breaking change». Nothing
 * could enforce that from TypeScript, and nothing did: by the time the package
 * was deleted the table named nine variants the lexer no longer had
 * (`KwPrefix`, `KwIn`, `KwUse`, `KwAs`, `KwIri`, `Template`, `AbsIri`, `EnvVar`,
 * `Pipe`), was missing three it had gained (`True`, `False`, `Null`), and every
 * discriminant from 3 upwards pointed at the wrong token — silently, because a
 * wrong colour is not an exception.
 *
 * So the numbers are not the contract any more. `fossil-wasm` ships
 * `tokenKinds()`, a legend of variant names indexed by the discriminant a row
 * carries, and this file maps NAMES. A reorder of the Rust enum now moves both
 * sides at once and changes nothing here. A variant appended by a newer compiler
 * is a name this table does not have, which is `undefined`, which is plain text.
 *
 * ## What is deliberately not coloured
 *
 * `Ident` gets no tag. The lexer cannot tell a type from a binding from a column
 * — `fossil-ide`'s semantic tokens can, and the legend for those is a separate
 * export (`semanticLegend`) over a separate call. Painting every identifier one
 * colour here would be a guess that a later overlay has to undo.
 */
import { tags, type Tag } from '@lezer/highlight';

/**
 * The map. Keys are `fossil_syntax::lexer::Token` variant names exactly as
 * `tokenKinds()` reports them.
 */
export const TAG_BY_NAME: Readonly<Record<string, Tag>> = {
  // Trivia. `Whitespace` and `Newline` are absent on purpose: the lexer keeps
  // them so the indent pass can see them, and a decoration over a space is a
  // range CodeMirror has to maintain for no visible result.
  Comment: tags.lineComment,

  // The four reserved words. `grammar.bnf`'s KEYWORD production is the list.
  KwFrom: tags.keyword,
  KwAnd: tags.logicOperator,
  KwOr: tags.logicOperator,
  KwNot: tags.logicOperator,

  // `@subject`, `@rename` — a marker, not a value.
  AtAttr: tags.annotation,

  // Literals. `true`/`false`/`null` are literals rather than keywords in this
  // lexer, and that is not a technicality: they became tokens because leaving
  // them as `Ident` made `verified = true` report `unknown column \`true\``.
  True: tags.bool,
  False: tags.bool,
  Null: tags.null,
  Float: tags.float,
  Integer: tags.integer,
  String: tags.string,

  // `Ident` — see the header. No tag, on purpose.

  // Operators.
  Define: tags.definitionOperator,
  Eq: tags.compareOperator,
  Neq: tags.compareOperator,
  Le: tags.compareOperator,
  Ge: tags.compareOperator,
  Lt: tags.compareOperator,
  Gt: tags.compareOperator,
  Assign: tags.definitionOperator,
  Plus: tags.arithmeticOperator,
  Minus: tags.arithmeticOperator,
  Star: tags.arithmeticOperator,
  Slash: tags.arithmeticOperator,
  Percent: tags.arithmeticOperator,
  Question: tags.operator,

  // Punctuation.
  Colon: tags.punctuation,
  Dot: tags.derefOperator,
  Comma: tags.separator,
  LParen: tags.paren,
  RParen: tags.paren,
  LBrace: tags.brace,
  RBrace: tags.brace,
};

/**
 * The tag for one row's `kind`, given the legend the wasm module reported.
 *
 * Returns `null` for whitespace, for identifiers, and for any kind the legend
 * does not name — all three of which CodeMirror renders as plain text.
 */
export function tagFor(legend: readonly string[], kind: number): Tag | null {
  const name = legend[kind];
  if (name === undefined) return null;
  return TAG_BY_NAME[name] ?? null;
}
