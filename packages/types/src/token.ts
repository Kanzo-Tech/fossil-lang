/**
 * One token row produced by the WASM `tokenize(text)` export from
 * `@fossil-lang/wasm`. `kind` is the numeric SyntaxKind tag (cast from
 * `fossil_syntax::lexer::Token as u32`). `start` + `end` are UTF-8 byte
 * offsets into the source string.
 *
 * The `kind` → CodeMirror highlight category mapping lives in
 * `@fossil-lang/codemirror-fossil/src/tags.ts`. Per ADR-0030, appending new
 * Token variants is backwards-compatible; reordering existing variants is
 * a breaking change for downstream consumers and must bump the major version
 * in a coordinated release.
 */
export interface TokenRow {
  /** Numeric SyntaxKind tag (cast from `fossil_syntax::lexer::Token as u32`). */
  kind: number;
  /** Inclusive start byte offset (UTF-8) into the source string. */
  start: number;
  /** Exclusive end byte offset (UTF-8) into the source string. */
  end: number;
}

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
