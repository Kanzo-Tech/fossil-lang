/**
 * Lexer token NAME, and semantic-token kind NAME, → `@lezer/highlight` tag.
 *
 * A token's name is `TokenKind`, generated from the lexer's enum, so a key here that
 * the lexer does not have fails the type-check.
 *
 * ## What is deliberately not coloured
 *
 * `Ident` gets no lexical tag. The lexer cannot tell a type from a binding from a
 * column — `fossil-ide`'s semantic tokens can, and {@link SEMANTIC_TAG_BY_KIND}
 * is how they are painted over this table. Painting every identifier one colour
 * here would be a guess the overlay has to undo.
 */
import type { TokenKind } from '@fossil-lang/types';
import { tags, type Tag } from '@lezer/highlight';

/** The map. A token it does not name is plain text. */
export const TAG_BY_NAME: Readonly<Partial<Record<TokenKind, Tag>>> = {
  // Trivia. `Whitespace` and `Newline` are absent on purpose: the lexer keeps
  // them so the indent pass can see them, and a decoration over a space is a
  // range CodeMirror has to maintain for no visible result.
  Comment: tags.lineComment,

  // The four reserved words. `grammar.bnf`'s KEYWORD production is the list.
  KwFrom: tags.keyword,
  KwAnd: tags.logicOperator,
  KwOr: tags.logicOperator,
  KwNot: tags.logicOperator,

  // `@subject`, `@rename` — a marker naming what the line is for. Not
  // `annotation`: themes draw that as faint, comment-adjacent text, and these
  // are the most load-bearing words in a mapping.
  AtAttr: tags.special(tags.variableName),

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
 * Semantic-token kind → tag, keyed by the legend NAMES `semanticTokens()` sends.
 *
 * `declaration` wraps the tag in `tags.definition`, so a binding where it is
 * made can be styled apart from its uses and falls back to the plain tag when the
 * theme does not.
 *
 * `string`, `number`, `operator` and `comment` are absent: the lexer names those
 * already, and more finely (`integer` from `float`, `:=` from `==`).
 */
export const SEMANTIC_TAG_BY_KIND: Readonly<Record<string, Tag>> = {
  namespace: tags.namespace,
  type: tags.typeName,
  function: tags.function(tags.variableName),
  property: tags.propertyName,
  parameter: tags.attributeName,
  variable: tags.variableName,
  keyword: tags.keyword,
};

/**
 * Semantic kinds the lexer can also produce. A row of one of these paints only
 * where the lexer painted nothing — that is how `type` and `as`, identifiers to
 * the lexer, become keywords without `and` losing its `logicOperator`.
 */
export const LEXICAL_KINDS: ReadonlySet<string> = new Set([
  'keyword',
  'string',
  'number',
  'operator',
  'comment',
]);

/**
 * The tag for one semantic row, or `null` for a kind {@link SEMANTIC_TAG_BY_KIND}
 * does not name — which leaves the lexical colour in place.
 */
export function semanticTagFor(kind: string, modifiers: readonly string[]): Tag | null {
  const tag = SEMANTIC_TAG_BY_KIND[kind];
  if (tag === undefined) return null;
  return modifiers.includes('declaration') ? tags.definition(tag) : tag;
}
