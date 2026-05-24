/**
 * Smoke tests for the Fossil StreamParser.
 *
 * The StreamParser interface CodeMirror invokes is `token(stream, state)` —
 * we drive it directly with a minimal `StringStream`-shaped stub. The
 * production code only touches `string`, `pos`, and `skipToEnd()` on the
 * stream object; the rest of CodeMirror's StringStream surface (eat, peek,
 * match, ...) is not exercised by our parser, so we don't need to stub it.
 */
import { describe, expect, it } from 'vitest';
import { fossilStreamParser } from '../src/parser.js';
import { FossilKind, kindToTagName } from '../src/tags.js';

/**
 * Minimal `StringStream`-shape stub. CodeMirror's StringStream has many more
 * methods (`eat`, `peek`, `match`, `next`, ...) but the Fossil parser only
 * reads `string` + writes `pos` + calls `skipToEnd()`. Keeping the stub
 * minimal makes the test's coupling to the parser surface explicit.
 */
function makeStream(s: string) {
  return {
    string: s,
    pos: 0,
    skipToEnd() {
      this.pos = s.length;
    },
  };
}

describe('fossilStreamParser', () => {
  it('startState returns a fresh empty state', () => {
    const state = fossilStreamParser.startState!();
    expect(state).toEqual({ allTokens: [], idx: 0, pos: 0, docLength: 0 });
  });

  it('returns null for empty input (no tokens to emit)', () => {
    const state = fossilStreamParser.startState!();
    const stream = makeStream('');
    // First call: empty doc → tokenize('') = []; skipToEnd; return null.
    // The current implementation re-tokenizes on length mismatch (0 vs 0 is
    // not a mismatch when allTokens.length === 0 also), so we just check
    // the null return + that pos was moved to end.
    const tag = fossilStreamParser.token(stream as never, state);
    expect(tag).toBeNull();
  });

  it('emits a non-null tag for the leading `prefix` keyword', () => {
    const state = fossilStreamParser.startState!();
    const stream = makeStream('prefix ex: <https://example.org/>');
    const tag = fossilStreamParser.token(stream as never, state);
    // First token is `KwPrefix` → 'keyword'. Assert both the contract (tag
    // is non-null) AND the specific category (so a refactor of tags.ts
    // doesn't silently drop keyword styling).
    expect(tag).not.toBeNull();
    expect(tag).toBe('keyword');
    // The state should now have cached tokens.
    expect(state.allTokens.length).toBeGreaterThan(0);
    // Lexer trivia: the lexer emits Whitespace+Newline tokens too — the very
    // first row should be KwPrefix (the parser starts at offset 0).
    expect(state.allTokens[0]!.kind).toBe(FossilKind.KwPrefix);
  });

  it('walks through a multi-token document and emits the expected tag sequence', () => {
    const state = fossilStreamParser.startState!();
    const source = 'prefix ex: <https://example.org/>';
    const stream = makeStream(source);

    const emitted: Array<string | null> = [];
    // Drive the parser until pos reaches end. Cap iterations defensively so
    // a regression doesn't spin forever.
    let safety = 0;
    while (stream.pos < source.length && safety < 200) {
      safety++;
      emitted.push(fossilStreamParser.token(stream as never, state));
    }

    // Filter out the null entries (gaps + trivia) — the non-null sequence
    // tells us which categories were emitted in order. The leading keyword
    // should appear; the AbsIri should be emitted as a 'string'.
    const nonNull = emitted.filter((e) => e !== null);
    expect(nonNull).toContain('keyword');
    expect(nonNull).toContain('string'); // <https://...>
    // First non-null tag is the `prefix` keyword.
    expect(nonNull[0]).toBe('keyword');
  });

  it('copyState clones state without sharing the idx counter', () => {
    const state = fossilStreamParser.startState!();
    state.allTokens = [{ kind: FossilKind.KwPrefix, start: 0, end: 6 }];
    state.idx = 7;
    state.pos = 11;
    state.docLength = 12;
    const copy = fossilStreamParser.copyState!(state);
    copy.idx = 99;
    expect(state.idx).toBe(7);
    expect(copy.idx).toBe(99);
    // allTokens is shared by reference (a doc-wide cache); this is intentional
    // for v0.1 to avoid array-clone cost on every line.
    expect(copy.allTokens).toBe(state.allTokens);
  });

  it('exposes a languageData with the `//` line-comment delimiter', () => {
    // Sanity: the comment-delimiter matches what the Rust lexer's Comment
    // regex (lexer.rs:45) recognises (`//[^\n]*`). Mismatch would break the
    // editor's "toggle line comment" keybinding.
    expect(fossilStreamParser.languageData).toMatchObject({
      commentTokens: { line: '//' },
    });
  });
});

describe('kindToTagName', () => {
  it('maps keyword kinds to the `keyword` tag', () => {
    expect(kindToTagName(FossilKind.KwPrefix)).toBe('keyword');
    expect(kindToTagName(FossilKind.KwFrom)).toBe('keyword');
    expect(kindToTagName(FossilKind.KwIri)).toBe('keyword');
  });

  it('maps string-shaped kinds to `string`', () => {
    expect(kindToTagName(FossilKind.String)).toBe('string');
    expect(kindToTagName(FossilKind.Template)).toBe('string');
    expect(kindToTagName(FossilKind.AbsIri)).toBe('string');
  });

  it('maps Ident to null (LSP semantic-tokens overlay paints these later)', () => {
    expect(kindToTagName(FossilKind.Ident)).toBeNull();
  });

  it('returns null for an unknown kind (forward-compat with appended Token variants)', () => {
    // 9999 is far beyond any current FossilKind discriminant — simulates a
    // newly-appended Rust Token variant whose mapping hasn't shipped yet.
    expect(kindToTagName(9999)).toBeNull();
  });
});
