/**
 * One token row produced by the WASM `tokenize(text)` export from
 * `@fossil-lang/wasm`. `kind` is the numeric tag (cast from
 * `fossil_syntax::lexer::Token as u32`). `start` + `end` are UTF-8 byte
 * offsets into the source string.
 *
 * **`kind` is meaningless without {@link TokenKindLegend}.** It is a variant
 * discriminant, so a reorder of the Rust enum remaps every value silently.
 * This comment used to point at a table in `codemirror-fossil` that «must bump
 * the major version in a coordinated release»; nothing could hold that, the
 * package was deleted in `873cbc0`, and by then the table was wrong in nine
 * places. Read the name — `tokenKinds()[row.kind]` — and the reorder stops
 * mattering.
 */
export interface TokenRow {
  /** Numeric tag (cast from `fossil_syntax::lexer::Token as u32`). Index it
   *  into {@link TokenKindLegend} rather than comparing it to a literal. */
  kind: number;
  /** Inclusive start byte offset (UTF-8) into the source string. */
  start: number;
  /** Exclusive end byte offset (UTF-8) into the source string. */
  end: number;
}

/**
 * Every lexer variant NAME, indexed by the {@link TokenRow.kind} that carries
 * it — the return of `@fossil-lang/wasm`'s `tokenKinds()`.
 *
 * `legend[row.kind]` is `"Comment"`, `"KwFrom"`, `"String"`, … and `undefined`
 * for a variant appended by a compiler newer than this host, which is the
 * backwards-compatible case: an unknown kind styles as plain text.
 */
export type TokenKindLegend = readonly string[];

/**
 * LSP `SemanticTokensLegend` shape (matches `fossil-wasm::semantic_legend()`
 * return). Identifies the token-type + token-modifier names used in the
 * accompanying semantic-tokens encoded data stream.
 */
export interface SemanticTokensLegend {
  /** Token-type names (e.g., `"keyword"`, `"variable"`, `"property"`). */
  tokenTypes: string[];
  /** Token-modifier names (e.g., `"declaration"`, `"readonly"`). */
  tokenModifiers: string[];
}
