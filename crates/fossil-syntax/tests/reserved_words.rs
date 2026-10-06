//! `grammar.bnf`, § RESERVED KEYWORDS, against the lexer: every word the
//! section reserves lexes as something other than an identifier, and every
//! word it names NOT KEYWORDS lexes as one.

use fossil_syntax::lexer::{Token, raw_lex};

/// The section's comment lines, delimiters stripped.
fn section() -> Vec<String> {
    let grammar = include_str!("../../../grammar.bnf");
    grammar
        .lines()
        .skip_while(|l| !l.contains("═══ RESERVED KEYWORDS"))
        .skip(1)
        .take_while(|l| !l.contains('═'))
        .filter_map(|l| {
            let t = l.trim().strip_prefix("(*")?.strip_suffix("*)")?;
            Some(t.trim().to_owned())
        })
        .collect()
}

fn lexes_as_ident(word: &str) -> bool {
    raw_lex(word).iter().map(|(t, _)| t).eq([&Token::Ident])
}

#[test]
fn the_lexer_reserves_exactly_what_the_grammar_reserves() {
    let lines = section();
    let reserved: Vec<&str> = lines
        .iter()
        .find(|l| !l.is_empty())
        .map(|l| l.split_whitespace().collect())
        .unwrap_or_default();
    assert!(reserved.contains(&"from"), "{reserved:?}");
    for word in &reserved {
        assert!(
            !lexes_as_ident(word),
            "grammar.bnf reserves `{word}` and it lexes as IDENT"
        );
    }

    let joined = lines.join(" ");
    let (_, rest) = joined
        .split_once("NOT KEYWORDS")
        .expect("the NOT KEYWORDS sentence");
    let claim = rest.split_once(". Every").map_or(rest, |(c, _)| c);
    let ordinary: Vec<&str> = claim.split('`').skip(1).step_by(2).collect();
    assert!(ordinary.contains(&"where"), "{ordinary:?}");
    for word in &ordinary {
        assert!(
            lexes_as_ident(word),
            "grammar.bnf says `{word}` is not a keyword and the lexer reserves it"
        );
    }
}
