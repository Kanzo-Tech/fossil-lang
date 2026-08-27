/**
 * The one arithmetic that has to be right: UTF-8 byte offsets → UTF-16 code
 * units.
 *
 * `tokenize()` returns byte offsets, because the Rust lexer indexes a `&str`.
 * CodeMirror indexes a JavaScript string, which is UTF-16 code units. They agree
 * for ASCII and diverge for everything else — one accented character shifts every
 * later token by one, and a decoration placed on a shifted range does not throw,
 * it just colours the wrong text.
 *
 * `fossil-wasm` says the same thing from the other side: `TokenRow`'s doc comment
 * notes «a JS host converts to UTF-16 code-unit offsets via `LineIndex` if it
 * needs them — a highlighter running over the same source string does not». That
 * last clause holds only for ASCII, and `tokenize_handles_unicode_in_comments` is
 * a test in that crate precisely because non-ASCII sources exist.
 *
 * Diagnostics do NOT come through here. `CheckRow.range` is already UTF-16 — the
 * compiler put it through `fossil_ide::LineIndex` on the Rust side, the
 * rust-analyzer model, because LSP will not accept anything else.
 */

/** ASCII-only text needs no map, and most programs are ASCII-only. */
const NON_ASCII = /[^\x00-\x7f]/;

/**
 * A byte-offset → UTF-16-offset function for one document.
 *
 * The identity function when the text is ASCII — checked once with a regex, so
 * the common path builds nothing and every lookup is free.
 *
 * Otherwise: `delta = byteOffset - unitOffset` is monotonically non-decreasing,
 * and it only changes at a character that is not one byte wide. Recording those
 * change points and binary-searching them is O(log k) per token for k such
 * characters, against the O(n) per-byte array the obvious version would build for
 * a document whose interesting characters are a handful of accents in a comment.
 */
export function byteToUtf16Mapper(text: string): (byte: number) => number {
  if (!NON_ASCII.test(text)) return (byte) => byte;

  /** Byte offset just past each character that widened the delta. */
  const at: number[] = [];
  /** The delta in force from that byte offset onwards. */
  const delta: number[] = [];

  let byte = 0;
  let unit = 0;
  for (let i = 0; i < text.length; ) {
    const code = text.codePointAt(i)!;
    const unitWidth = code > 0xffff ? 2 : 1;
    const byteWidth = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    byte += byteWidth;
    unit += unitWidth;
    i += unitWidth;
    if (byteWidth !== unitWidth) {
      at.push(byte);
      delta.push(byte - unit);
    }
  }

  return (target: number): number => {
    // The last change point at or before `target` fixes the delta there.
    let lo = 0;
    let hi = at.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (at[mid]! <= target) {
        found = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    const shift = found === -1 ? 0 : delta[found]!;
    // Clamp: a byte offset inside a multi-byte character cannot happen at a token
    // boundary, and if it ever does, landing on the character start beats
    // throwing inside a view plugin.
    return Math.max(0, Math.min(target - shift, text.length));
  };
}
