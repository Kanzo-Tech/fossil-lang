/**
 * LSP-shaped diagnostic types — mirrors the shape that `fossil-wasm`'s
 * `CheckRow` returns (see `crates/fossil-wasm/src/lib.rs`). The numeric
 * severity is the LSP integer constant:
 *
 *   1 = Error, 2 = Warning, 3 = Information, 4 = Hint.
 *
 * `@fossil-lang/codemirror-fossil` maps these to CodeMirror lint severities;
 * `@fossil-lang/playground` renders them in the diagnostics panel.
 */

/** Zero-based line + UTF-16 character column (LSP convention). */
export interface Position {
  line: number;
  character: number;
}

/** Inclusive `start`, exclusive `end` range (LSP convention). */
export interface Range {
  start: Position;
  end: Position;
}

/**
 * A single LSP-shaped diagnostic from the Fossil compiler / LSP / WASM.
 * Mirrors `crates/fossil-wasm/src/lib.rs::CheckRow`.
 */
export interface Diagnostic {
  /** Document URI the diagnostic applies to. */
  uri: string;
  /** Range within the document. */
  range: Range;
  /** LSP integer severity constant (1=Error, 2=Warning, 3=Info, 4=Hint). */
  severity: 1 | 2 | 3 | 4;
  /** Human-readable diagnostic message. */
  message: string;
  /** Optional source identifier (e.g., `"fossil-typecheck"`). */
  source?: string;
  /** Optional diagnostic code (string or number — LSP allows both). */
  code?: string | number;
}
