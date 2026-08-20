//! Post-lexer pass — converts the raw `Token` stream from [`crate::lexer`]
//! into a parser-ready stream of [`LexedToken`]s, inserting virtual
//! `INDENT`/`DEDENT` tokens at column changes after newlines.
//!
//! Algorithm (Python / rust-analyzer pattern, ~80 LOC core):
//!
//! 1. Maintain a stack of indentation columns; the bottom is `0`.
//! 2. At every `Newline`, mark the next token as line-start.
//! 3. When the next non-trivia token appears at line-start, measure its column.
//!    - If `col > top`: push `col`, emit `INDENT`.
//!    - If `col == top`: emit nothing.
//!    - If `col < top`: pop until `top == col`, emitting `DEDENT` per pop.
//!      If no level matches, emit `ERROR` — the parser recovers from it rather
//!      than the lexer guessing which level was meant.
//! 4. Blank lines and comment-only lines do NOT change the stack.
//! 5. At EOF, emit `DEDENT` for every nonzero stack entry.
//! 6. A byte logos matched no rule for becomes an `ERROR` token carrying that
//!    byte. It counts as a real token for step 3, so a stray character at
//!    column 0 closes the block above it exactly as any other token would.
//!
//! INDENT/DEDENT lexing is the classic week-eater in an indentation-sensitive
//! language, and it was named the highest-overrun risk here before a line of it
//! existed. The 3-fixture corpus at the bottom of this file is the gate.

use crate::kind::SyntaxKind;
use crate::lexer::{Token, raw_lex_lossless};

/// One token in the parser-ready stream — kind, original source text, and span.
///
/// Virtual `INDENT`/`DEDENT` tokens carry an empty `text` and a zero-length
/// span at the position where the indent change was observed.
#[derive(Debug, Clone)]
pub struct LexedToken {
    pub kind: SyntaxKind,
    pub text: String,
    pub range: std::ops::Range<usize>,
}

/// Run the post-lexer pass over `input`, producing the parser-ready stream.
///
/// See module docs for the algorithm. The output preserves ALL source bytes —
/// whitespace, newlines and comments become their `SyntaxKind` trivia variants,
/// and a byte no lexer rule matched becomes a `SyntaxKind::ERROR` token
/// carrying that byte as its text. That last one is what makes the CST lossless
/// for a file with a stray `#` in it, and it is what gives the parser something
/// to attach a diagnostic to; `SyntaxKind::ERROR` is deliberately NOT trivia in
/// `Parser::skip_trivia`, so it cannot be skipped back into silence.
#[must_use]
pub fn lex_with_indents(input: &str) -> Vec<LexedToken> {
    let raw = raw_lex_lossless(input);
    let mut out = Vec::with_capacity(raw.len() + 8);
    let mut indent_stack: Vec<usize> = vec![0];
    let mut at_line_start = true;
    let mut i = 0;

    while i < raw.len() {
        let (tok, range) = raw[i].clone();

        match tok {
            Some(Token::Newline) => {
                push_simple(&mut out, SyntaxKind::NEWLINE, &input[range.clone()], range);
                at_line_start = true;
                i += 1;
            }
            Some(Token::Whitespace) if at_line_start => {
                let col = range.end - range.start;
                // Emit the leading whitespace as trivia so the CST stays lossless.
                push_simple(
                    &mut out,
                    SyntaxKind::WHITESPACE,
                    &input[range.clone()],
                    range.clone(),
                );
                i += 1;

                // Blank-line / comment-only-line guard: don't change the stack.
                let next = raw.get(i).map(|(t, _)| *t);
                if matches!(next, Some(Some(Token::Newline | Token::Comment))) {
                    continue;
                }
                if next.is_none() {
                    // Trailing whitespace before EOF; let the EOF dedent loop fire.
                    continue;
                }

                emit_indent_changes(&mut out, &mut indent_stack, col, range.start);
                at_line_start = false;
            }
            Some(Token::Whitespace) => {
                push_simple(
                    &mut out,
                    SyntaxKind::WHITESPACE,
                    &input[range.clone()],
                    range,
                );
                i += 1;
            }
            Some(Token::Comment) => {
                push_simple(&mut out, SyntaxKind::COMMENT, &input[range.clone()], range);
                i += 1;
            }
            // Every other token, and `None` — the bytes logos rejected. An
            // unlexable byte is treated as a real token here on purpose: it
            // opens or closes a block like any other (a `#` at column 0 after
            // an indented body is still a dedent), and it is emitted as an
            // ERROR token carrying its own text, which the parser reports.
            _ => {
                if at_line_start {
                    // Line starts at column 0 with a real token — possibly a dedent.
                    emit_indent_changes(&mut out, &mut indent_stack, 0, range.start);
                    at_line_start = false;
                }
                let carved = match tok {
                    Some(Token::String) => carve_interpolations(&mut out, input, &range),
                    _ => false,
                };
                if !carved {
                    let kind = tok.map_or(SyntaxKind::ERROR, token_to_kind);
                    push_simple(&mut out, kind, &input[range.clone()], range);
                }
                i += 1;
            }
        }
    }

    // EOF: emit DEDENT for every indent still on the stack.
    while indent_stack.len() > 1 {
        indent_stack.pop();
        out.push(LexedToken {
            kind: SyntaxKind::DEDENT,
            text: String::new(),
            range: input.len()..input.len(),
        });
    }
    out
}

/// Carve a string literal that has at least one hole into the token run
/// `STRING_OPEN (STRING_TEXT | INTERP_OPEN <expr tokens> RBRACE)* STRING_CLOSE`,
/// returning whether it carved.
///
/// This is PEP 701's division of labour, and the reason it is here rather than
/// in `lexer.rs`: the lexer finds the boundaries, and everything between them
/// is re-lexed into ORDINARY tokens so the ordinary expression parser reads it.
/// Python spent seven years leaving f-strings to a hand-written post-pass over
/// STRING tokens; the rule here is not to repeat that, and the
/// cost of repeating it is measurable in this tree — see the `lower_placeholder`
/// this commit deletes, which parsed holes at MIR-lowering time and echoed back
/// as literal text anything it did not recognise.
///
/// The other reason it is here and not in `lexer.rs`: `fossil-wasm`'s
/// `tokenize` feeds the editor from `raw_lex` DIRECTLY, and its `Token`
/// discriminants are pinned across the language boundary to
/// `packages/codemirror-fossil/src/tags.ts`. Only the parser reads this pass,
/// so carving here leaves that pin untouched.
///
/// There is ONE opener, `{`. It took an `open_marker` parameter while the
/// backtick spelling's `${` existed; `"…{expr}…"` is the spelling, `${` is not
/// a token, and the parameter went with the second one.
fn carve_interpolations(
    out: &mut Vec<LexedToken>,
    input: &str,
    range: &std::ops::Range<usize>,
) -> bool {
    const OPEN_MARKER: &str = "{";
    let text = &input[range.clone()];
    let (Some(open), Some(close)) = (text.get(..1), text.len().checked_sub(1)) else {
        return false;
    };
    if close < 1 {
        return false; // not a delimited literal at all
    }
    let inner = &text[1..close];
    let base = range.start + 1;
    let Some(first) = find_hole(inner, OPEN_MARKER) else {
        return false; // no hole — it stays the single token it has always been
    };
    let open_marker = OPEN_MARKER;

    push_simple(out, SyntaxKind::STRING_OPEN, open, range.start..base);

    let mut cursor = 0usize;
    let mut hole = Some(first);
    while let Some(at) = hole {
        if at > cursor {
            push_simple(
                out,
                SyntaxKind::STRING_TEXT,
                &inner[cursor..at],
                base + cursor..base + at,
            );
        }
        let body_start = at + open_marker.len();
        push_simple(
            out,
            SyntaxKind::INTERP_OPEN,
            open_marker,
            base + at..base + body_start,
        );

        // The hole's body: re-lexed into ordinary tokens, spans kept absolute.
        // Lossless, like the outer pass — a stray byte inside a hole is an
        // ERROR token the parser reports, not a byte that disappears from a
        // tree whose whole job is to hold every byte.
        let body_end = match_closing_brace(inner, body_start);
        for (tok, r) in raw_lex_lossless(&inner[body_start..body_end]) {
            push_simple(
                out,
                tok.map_or(SyntaxKind::ERROR, token_to_kind),
                &inner[body_start + r.start..body_start + r.end],
                base + body_start + r.start..base + body_start + r.end,
            );
        }
        if body_end < inner.len() {
            push_simple(
                out,
                SyntaxKind::RBRACE,
                "}",
                base + body_end..base + body_end + 1,
            );
            cursor = body_end + 1;
        } else {
            // Unterminated hole. Nothing is invented here: the run stops and
            // the parser's `expect(RBRACE)` reports it, which is the whole
            // point of the hole being the parser's business.
            cursor = body_end;
        }
        hole = find_hole(&inner[cursor..], open_marker).map(|h| cursor + h);
    }
    if cursor < inner.len() {
        push_simple(
            out,
            SyntaxKind::STRING_TEXT,
            &inner[cursor..],
            base + cursor..base + inner.len(),
        );
    }
    push_simple(
        out,
        SyntaxKind::STRING_CLOSE,
        &text[close..],
        range.start + close..range.end,
    );
    true
}

/// Byte offset of the next hole opener in `s`, skipping `{{` — the escape Rust,
/// Python and C# all spell the same way.
fn find_hole(s: &str, open_marker: &str) -> Option<usize> {
    let mut i = 0usize;
    while i < s.len() {
        if s[i..].starts_with("{{") {
            i += 2;
            continue;
        }
        if s[i..].starts_with(open_marker) {
            return Some(i);
        }
        i += 1;
        while !s.is_char_boundary(i) {
            i += 1;
        }
    }
    None
}

/// Byte offset of the `}` that closes the hole opened before `from`, counting
/// nesting and skipping quoted strings so a record literal or a string argument
/// inside a hole does not end it early. Returns `s.len()` when there is none.
fn match_closing_brace(s: &str, from: usize) -> usize {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'{' => depth += 1,
            b'}' if depth == 0 => return i,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    s.len()
}

fn emit_indent_changes(out: &mut Vec<LexedToken>, stack: &mut Vec<usize>, col: usize, pos: usize) {
    let top = *stack
        .last()
        .expect("indent stack must always have a 0 sentinel");
    if col > top {
        stack.push(col);
        out.push(LexedToken {
            kind: SyntaxKind::INDENT,
            text: String::new(),
            range: pos..pos,
        });
    } else {
        while *stack
            .last()
            .expect("indent stack must always have a 0 sentinel")
            > col
        {
            stack.pop();
            out.push(LexedToken {
                kind: SyntaxKind::DEDENT,
                text: String::new(),
                range: pos..pos,
            });
        }
        if *stack
            .last()
            .expect("indent stack must always have a 0 sentinel")
            != col
        {
            // Inconsistent dedent — emit ERROR; parser recovers in later phases.
            out.push(LexedToken {
                kind: SyntaxKind::ERROR,
                text: String::new(),
                range: pos..pos,
            });
        }
    }
}

fn push_simple(
    out: &mut Vec<LexedToken>,
    kind: SyntaxKind,
    text: &str,
    range: std::ops::Range<usize>,
) {
    out.push(LexedToken {
        kind,
        text: text.to_string(),
        range,
    });
}

const fn token_to_kind(t: Token) -> SyntaxKind {
    match t {
        // Trivia
        Token::Whitespace => SyntaxKind::WHITESPACE,
        Token::Newline => SyntaxKind::NEWLINE,
        Token::Comment => SyntaxKind::COMMENT,
        // Keywords
        Token::KwFrom => SyntaxKind::KW_FROM,
        Token::KwAnd => SyntaxKind::KW_AND,
        Token::KwOr => SyntaxKind::KW_OR,
        Token::KwNot => SyntaxKind::KW_NOT,
        // Lexical
        Token::Ident => SyntaxKind::IDENT,
        Token::Integer => SyntaxKind::INTEGER,
        Token::Float => SyntaxKind::FLOAT,
        // Two lexer tokens, ONE kind: the value is the token's text, and the
        // parser wants «a boolean literal is here», not «which one».
        Token::True | Token::False => SyntaxKind::BOOL,
        Token::String => SyntaxKind::STRING,
        Token::AtAttr => SyntaxKind::AT_ATTR,
        // Multi-char operators
        Token::Define => SyntaxKind::DEFINE,
        Token::Eq => SyntaxKind::EQ,
        Token::Neq => SyntaxKind::NEQ,
        Token::Le => SyntaxKind::LE,
        Token::Ge => SyntaxKind::GE,
        // Single-char operators / punctuation
        Token::Assign => SyntaxKind::ASSIGN,
        Token::Colon => SyntaxKind::SHAPE_SEP,
        Token::Dot => SyntaxKind::DOT,
        Token::Comma => SyntaxKind::COMMA,
        Token::LParen => SyntaxKind::LPAREN,
        Token::RParen => SyntaxKind::RPAREN,
        Token::LBrace => SyntaxKind::LBRACE,
        Token::RBrace => SyntaxKind::RBRACE,
        Token::Lt => SyntaxKind::LT,
        Token::Gt => SyntaxKind::GT,
        Token::Plus => SyntaxKind::PLUS,
        Token::Minus => SyntaxKind::MINUS,
        Token::Star => SyntaxKind::STAR,
        Token::Slash => SyntaxKind::SLASH,
        Token::Percent => SyntaxKind::PERCENT,
        Token::Question => SyntaxKind::T_QUESTION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::SyntaxKind::{
        ASSIGN, AT_ATTR, COMMENT, DEDENT, DEFINE, DOT, ERROR, IDENT, INDENT, LPAREN, NEWLINE,
        RPAREN, STRING, WHITESPACE,
    };

    /// Filter trivia (whitespace / newlines / comments) from the token stream
    /// so we only assert the structural shape produced by the indent pass.
    fn kinds(input: &str) -> Vec<SyntaxKind> {
        lex_with_indents(input)
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| !matches!(k, WHITESPACE | NEWLINE | COMMENT))
            .collect()
    }

    #[test]
    fn flat_no_indents() {
        let input = "User := io.csv(\"u.csv\")\n";
        assert_eq!(
            kinds(input),
            vec![IDENT, DEFINE, IDENT, DOT, IDENT, LPAREN, STRING, RPAREN]
        );
    }

    #[test]
    fn one_indent_one_dedent() {
        let input = "User\n    @subject = User.id\n";
        assert_eq!(
            kinds(input),
            vec![IDENT, INDENT, AT_ATTR, ASSIGN, IDENT, DOT, IDENT, DEDENT]
        );
    }

    #[test]
    fn unlexable_byte_becomes_an_error_token_carrying_its_text() {
        let out = lex_with_indents("#");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, ERROR);
        assert_eq!(out[0].text, "#");
        assert_eq!(out[0].range, 0..1);
        // And the stream is still lossless — every byte of the input is in it.
        let input = "a #\n";
        let rebuilt: String = lex_with_indents(input)
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(rebuilt, input);
    }

    #[test]
    fn unlexable_byte_at_column_zero_closes_the_block() {
        // It counts as a real token for the indent stack: the mapping body
        // above it is closed, exactly as an IDENT there would close it.
        let input = "User\n    @subject = User.id\n#\n";
        assert_eq!(
            kinds(input),
            vec![
                IDENT, INDENT, AT_ATTR, ASSIGN, IDENT, DOT, IDENT, DEDENT, ERROR
            ]
        );
    }

    #[test]
    fn nested_indent_double_dedent() {
        let input = "A\n    B\n        C\nD\n";
        assert_eq!(
            kinds(input),
            vec![IDENT, INDENT, IDENT, INDENT, IDENT, DEDENT, DEDENT, IDENT]
        );
    }

    #[test]
    fn blank_line_does_not_affect_stack() {
        let input = "User\n    @subject = User.id\n\n    name = User.n\n";
        assert_eq!(
            kinds(input),
            vec![
                IDENT, INDENT, AT_ATTR, ASSIGN, IDENT, DOT, IDENT, IDENT, ASSIGN, IDENT, DOT,
                IDENT, DEDENT
            ]
        );
    }
}
