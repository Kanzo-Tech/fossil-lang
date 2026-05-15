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
//!      If no level matches, emit `ERROR` (parser recovers in Phase 2+).
//! 4. Blank lines and comment-only lines do NOT change the stack.
//! 5. At EOF, emit `DEDENT` for every nonzero stack entry.
//!
//! Per RESEARCH.md Pitfall 1 this is the highest-overrun risk in Phase 1;
//! the 3-fixture corpus at the bottom of this file is the gate.

use crate::kind::SyntaxKind;
use crate::lexer::{Token, raw_lex};

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
/// See module docs for the algorithm. The output preserves all source bytes
/// (whitespace, newlines, comments are emitted as their `SyntaxKind` trivia
/// variants) so the parser can build a lossless CST.
#[must_use]
pub fn lex_with_indents(input: &str) -> Vec<LexedToken> {
    let raw = raw_lex(input);
    let mut out = Vec::with_capacity(raw.len() + 8);
    let mut indent_stack: Vec<usize> = vec![0];
    let mut at_line_start = true;
    let mut i = 0;

    while i < raw.len() {
        let (tok, range) = raw[i].clone();

        match tok {
            Token::Newline => {
                push_simple(&mut out, SyntaxKind::NEWLINE, &input[range.clone()], range);
                at_line_start = true;
                i += 1;
            }
            Token::Whitespace if at_line_start => {
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
                let next = raw.get(i).map(|(t, _)| t);
                if matches!(next, Some(Token::Newline | Token::Comment)) {
                    continue;
                }
                if next.is_none() {
                    // Trailing whitespace before EOF; let the EOF dedent loop fire.
                    continue;
                }

                emit_indent_changes(&mut out, &mut indent_stack, col, range.start);
                at_line_start = false;
            }
            Token::Whitespace => {
                push_simple(
                    &mut out,
                    SyntaxKind::WHITESPACE,
                    &input[range.clone()],
                    range,
                );
                i += 1;
            }
            Token::Comment => {
                push_simple(&mut out, SyntaxKind::COMMENT, &input[range.clone()], range);
                i += 1;
            }
            _ => {
                if at_line_start {
                    // Line starts at column 0 with a real token — possibly a dedent.
                    emit_indent_changes(&mut out, &mut indent_stack, 0, range.start);
                    at_line_start = false;
                }
                push_simple(&mut out, token_to_kind(tok), &input[range.clone()], range);
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
        Token::Whitespace => SyntaxKind::WHITESPACE,
        Token::Newline => SyntaxKind::NEWLINE,
        Token::Comment => SyntaxKind::COMMENT,
        Token::KwPrefix => SyntaxKind::KW_PREFIX,
        Token::KwFrom => SyntaxKind::KW_FROM,
        Token::Ident => SyntaxKind::IDENT,
        Token::Integer => SyntaxKind::INTEGER,
        Token::String => SyntaxKind::STRING,
        Token::Template => SyntaxKind::TEMPLATE,
        Token::AbsIri => SyntaxKind::ABS_IRI,
        Token::Define => SyntaxKind::DEFINE,
        Token::Assign => SyntaxKind::ASSIGN,
        Token::Colon => SyntaxKind::SHAPE_SEP,
        Token::Dot => SyntaxKind::DOT,
        Token::Comma => SyntaxKind::COMMA,
        Token::LParen => SyntaxKind::LPAREN,
        Token::RParen => SyntaxKind::RPAREN,
        Token::LBrace => SyntaxKind::LBRACE,
        Token::RBrace => SyntaxKind::RBRACE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::SyntaxKind::{
        ABS_IRI, ASSIGN, COMMENT, DEDENT, DOT, IDENT, INDENT, KW_PREFIX, NEWLINE, SHAPE_SEP,
        WHITESPACE,
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
        let input = "prefix ex: <a>\n";
        assert_eq!(kinds(input), vec![KW_PREFIX, IDENT, SHAPE_SEP, ABS_IRI]);
    }

    #[test]
    fn one_indent_one_dedent() {
        let input = "User\n    iri = .id\n";
        assert_eq!(
            kinds(input),
            vec![IDENT, INDENT, IDENT, ASSIGN, DOT, IDENT, DEDENT]
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
        let input = "User\n    iri = .id\n\n    name = .n\n";
        assert_eq!(
            kinds(input),
            vec![
                IDENT, INDENT, IDENT, ASSIGN, DOT, IDENT, IDENT, ASSIGN, DOT, IDENT, DEDENT
            ]
        );
    }
}
