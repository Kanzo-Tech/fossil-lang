//! **What a model may write** — the mappings of a program, as a grammar a constrained decoder
//! enforces token by token, whose only names are the ones that exist.
//!
//! A model asked for a mapping writes Fossil, the language, and is checked by the type checker,
//! the oracle (`/docs/design/profile`). What the checker cannot do is stop a name from being
//! invented: it reports `nmae` after the model wrote it. A decoder constrained by a grammar can —
//! `WebLLM`'s (`xgrammar`) takes an EBNF string — so the grammar for one request is the mapping
//! productions of `grammar.bnf`, in one canonical layout, with every reference a closed list: the
//! sources of the request, each one's columns, the shapes' types, each type's properties and the
//! catalogue's `parse.*` functions. A name not on a list cannot be decoded.
//!
//! What the grammar is NOT is a second definition of the language. It is a sub-language of it, and
//! the tests hold it there from both sides: every sentence sampled from it parses with the parser
//! every program goes through, and the conformance programs inside its scope are sentences of it.
//! Types, cardinality and identity are the checker's; a context-free grammar cannot say them.
//!
//! The prelude — the shapes imported, the sources bound — is not the model's to write: it is the
//! request, and [`prelude`] writes it. The model writes what follows.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a mapping can name: what a host knows before the model writes anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MappingVocabulary")]
pub struct Vocabulary {
    /// The shape document: its constructor (`shex`, `shacl`) and where it is.
    pub shapes: Document,
    /// The types it declares, each with the property keys a mapping of it may write.
    pub types: Vec<TypeVocabulary>,
    /// The sources, each with its columns — a profile's descriptor.
    pub sources: Vec<SourceVocabulary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "VocabularyDocument")]
pub struct Document {
    pub format: String,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TypeVocabulary")]
pub struct TypeVocabulary {
    pub name: String,
    pub properties: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SourceVocabulary")]
pub struct SourceVocabulary {
    /// The binding every reference to it is qualified by.
    pub name: String,
    pub format: String,
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    pub columns: Vec<String>,
}

/// One expression of the grammar. A small EBNF: enough for the mapping productions, and printed in
/// the dialect xgrammar reads (the W3C XML notation, `#` comments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Text(String),
    Rule(String),
    /// A character in these inclusive ranges, or — negated — any character outside them.
    Class {
        ranges: Vec<(char, char)>,
        negated: bool,
    },
    Seq(Vec<Expr>),
    Alt(Vec<Expr>),
    Star(Box<Expr>),
    Opt(Box<Expr>),
}

/// A grammar: named rules, the first of them the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grammar {
    pub rules: Vec<(String, Expr)>,
}

fn text(s: &str) -> Expr {
    Expr::Text(s.to_string())
}
fn rule(name: &str) -> Expr {
    Expr::Rule(name.to_string())
}
fn seq(items: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Seq(items.into_iter().collect())
}
fn alt(items: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Alt(items.into_iter().collect())
}
fn star(e: Expr) -> Expr {
    Expr::Star(Box::new(e))
}
fn opt(e: Expr) -> Expr {
    Expr::Opt(Box::new(e))
}
const fn class(ranges: Vec<(char, char)>, negated: bool) -> Expr {
    Expr::Class { ranges, negated }
}

/// `IDENT` in `grammar.bnf`, less the words that are tokens of their own.
fn is_ident(name: &str) -> bool {
    const KEYWORDS: [&str; 7] = ["from", "and", "or", "not", "true", "false", "null"];
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&name)
}

/// A string literal's content as the lexer reads it back: its escapes, and `{{` for a brace.
fn escaped(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '{' => out.push_str("{{"),
            c => out.push(c),
        }
    }
    out
}

/// `Source.column`, or `Source."a.column"` for a column that is not a name.
fn column(source: &str, column: &str) -> String {
    if is_ident(column) {
        format!("{source}.{column}")
    } else {
        format!("{source}.\"{}\"", escaped(column))
    }
}

/// What the request already says: the shapes imported and the sources bound. The model's mappings
/// follow it.
#[must_use]
pub fn prelude(vocabulary: &Vocabulary) -> String {
    let types: Vec<&str> = vocabulary.types.iter().map(|t| t.name.as_str()).collect();
    let mut out = format!(
        "type {{ {} }} := io.{}(\"{}\")\n",
        types.join(", "),
        vocabulary.shapes.format,
        escaped(&vocabulary.shapes.reference)
    );
    for source in &vocabulary.sources {
        let delimiter = source
            .delimiter
            .as_deref()
            .map(|d| format!(", delimiter = \"{}\"", escaped(d)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{} := io.{}(\"{}\"{delimiter})",
            source.name,
            source.format,
            escaped(&source.reference)
        );
    }
    out
}

/// The catalogue's `parse.*` functions, split by whether they take a format after the text.
fn parse_functions() -> (Vec<String>, Vec<String>) {
    let mut plain = Vec::new();
    let mut formatted = Vec::new();
    for entry in fossil_hir::stdlib::stdlib()
        .iter()
        .filter(|e| e.name.starts_with("parse."))
    {
        match entry.sig.params.len() {
            1 => plain.push(entry.name.to_string()),
            2 => formatted.push(entry.name.to_string()),
            _ => {}
        }
    }
    plain.sort();
    formatted.sort();
    (plain, formatted)
}

/// The grammar of the mappings a program over `vocabulary` can have: one or more, a blank line
/// between two, each `Name : Type from Source` with its `where`, its joins, `@subject` and the
/// type's properties, four spaces in, one `=` with a space either side.
#[must_use]
pub fn mapping_grammar(vocabulary: &Vocabulary) -> Grammar {
    let columns: Vec<Expr> = vocabulary
        .sources
        .iter()
        .flat_map(|s| s.columns.iter().map(|c| text(&column(&s.name, c))))
        .collect();
    // A hole in the subject reads a column by its name: one that is not a name has no spelling there.
    let named_columns: Vec<Expr> = vocabulary
        .sources
        .iter()
        .flat_map(|s| {
            s.columns
                .iter()
                .filter(|c| is_ident(c))
                .map(|c| text(&column(&s.name, c)))
        })
        .collect();
    let sources = alt(vocabulary.sources.iter().map(|s| text(&s.name)));
    let types = alt(vocabulary.types.iter().map(|t| text(&t.name)));
    let (plain, formatted) = parse_functions();
    let string_char = alt([
        class(
            vec![('"', '"'), ('\\', '\\'), ('{', '{'), ('\n', '\n')],
            true,
        ),
        text("\\\""),
        text("\\\\"),
        text("{{"),
    ]);

    let mut rules = vec![
        (
            "root".to_string(),
            seq([
                rule("map"),
                star(seq([text("\n\n"), rule("map")])),
                text("\n"),
            ]),
        ),
        (
            "map".to_string(),
            alt(vocabulary
                .types
                .iter()
                .map(|t| rule(&format!("map_{}", t.name)))),
        ),
    ];
    for t in &vocabulary.types {
        let keys = alt(t.properties.iter().map(|p| text(p)));
        rules.push((
            format!("map_{}", t.name),
            seq([
                rule("name"),
                text(&format!(" : {} from ", t.name)),
                rule("from"),
                text("\n"),
                rule("subject"),
                star(seq([text("\n    "), keys, text(" = "), rule("value")])),
            ]),
        ));
    }
    rules.extend([
        (
            "name".to_string(),
            seq([
                class(vec![('A', 'Z')], false),
                star(class(
                    vec![('A', 'Z'), ('a', 'z'), ('0', '9'), ('_', '_')],
                    false,
                )),
            ]),
        ),
        (
            "from".to_string(),
            seq([rule("source"), opt(rule("where")), star(rule("join"))]),
        ),
        ("source".to_string(), sources),
        (
            "where".to_string(),
            seq([
                text(".where("),
                rule("condition"),
                star(seq([text(" and "), rule("condition")])),
                text(")"),
            ]),
        ),
        (
            "condition".to_string(),
            seq([
                rule("column"),
                text(" "),
                alt(["==", "!=", "<=", ">=", "<", ">"].map(text)),
                text(" "),
                rule("literal"),
            ]),
        ),
        (
            "literal".to_string(),
            alt([
                text("null"),
                text("true"),
                text("false"),
                rule("integer"),
                rule("string"),
            ]),
        ),
        (
            "integer".to_string(),
            seq([
                opt(text("-")),
                class(vec![('0', '9')], false),
                star(class(vec![('0', '9')], false)),
            ]),
        ),
        (
            "string".to_string(),
            seq([text("\""), star(string_char.clone()), text("\"")]),
        ),
        (
            "join".to_string(),
            seq([
                text(".join("),
                rule("source"),
                text(", on = "),
                rule("column"),
                text(" == "),
                rule("column"),
                text(")"),
            ]),
        ),
        (
            "subject".to_string(),
            seq([
                text("    @subject = \""),
                star(alt([
                    string_char,
                    seq([text("{"), rule("named_column"), text("}")]),
                ])),
                text("\""),
            ]),
        ),
        (
            "value".to_string(),
            alt([
                rule("column"),
                rule("parse"),
                seq([rule("type"), text("("), rule("column"), text(")")]),
            ]),
        ),
        ("type".to_string(), types),
        ("column".to_string(), alt(columns)),
        ("named_column".to_string(), alt(named_columns)),
    ]);
    let mut parse = Vec::new();
    if !plain.is_empty() {
        parse.push(seq([
            alt(plain.iter().map(|f| text(f))),
            text("("),
            rule("column"),
            text(")"),
        ]));
    }
    if !formatted.is_empty() {
        parse.push(seq([
            alt(formatted.iter().map(|f| text(f))),
            text("("),
            rule("column"),
            text(", "),
            rule("string"),
            text(")"),
        ]));
    }
    rules.push(("parse".to_string(), alt(parse)));
    Grammar { rules }
}

impl Grammar {
    fn get(&self, name: &str) -> &Expr {
        &self
            .rules
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no rule `{name}`"))
            .1
    }

    /// The grammar in xgrammar's EBNF: `name ::= …`, the root first.
    #[must_use]
    pub fn ebnf(&self) -> String {
        fn literal(s: &str) -> String {
            let mut out = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        fn class_char(c: char) -> String {
            match c {
                '\n' => "\\n".into(),
                '\\' | ']' | '^' | '-' => format!("\\{c}"),
                c => c.to_string(),
            }
        }
        fn print(e: &Expr) -> String {
            match e {
                Expr::Text(s) => literal(s),
                Expr::Rule(r) => r.clone(),
                Expr::Class { ranges, negated } => {
                    let body: String = ranges
                        .iter()
                        .map(|(a, b)| {
                            if a == b {
                                class_char(*a)
                            } else {
                                format!("{}-{}", class_char(*a), class_char(*b))
                            }
                        })
                        .collect();
                    format!("[{}{body}]", if *negated { "^" } else { "" })
                }
                Expr::Seq(items) => items
                    .iter()
                    .map(print_grouped)
                    .collect::<Vec<_>>()
                    .join(" "),
                Expr::Alt(items) => items.iter().map(print).collect::<Vec<_>>().join(" | "),
                Expr::Star(e) => format!("{}*", print_grouped(e)),
                Expr::Opt(e) => format!("{}?", print_grouped(e)),
            }
        }
        fn print_grouped(e: &Expr) -> String {
            match e {
                Expr::Seq(_) | Expr::Alt(_) => format!("({})", print(e)),
                _ => print(e),
            }
        }
        let mut out = String::new();
        for (name, e) in &self.rules {
            let _ = writeln!(out, "{name} ::= {}", print(e));
        }
        out
    }

    /// Whether `input` is a sentence of the grammar, whole.
    #[must_use]
    pub fn matches(&self, input: &str) -> bool {
        let chars: Vec<char> = input.chars().collect();
        self.ends(self.get(&self.rules[0].0), &chars, 0)
            .contains(&chars.len())
    }

    /// Every position a match of `e` starting at `at` can end at.
    fn ends(&self, e: &Expr, input: &[char], at: usize) -> Vec<usize> {
        let mut out = match e {
            Expr::Text(s) => {
                let s: Vec<char> = s.chars().collect();
                if input[at..].starts_with(&s) {
                    vec![at + s.len()]
                } else {
                    vec![]
                }
            }
            Expr::Rule(r) => self.ends(self.get(r), input, at),
            Expr::Class { ranges, negated } => match input.get(at) {
                Some(c) if ranges.iter().any(|(a, b)| (a..=b).contains(&c)) != *negated => {
                    vec![at + 1]
                }
                _ => vec![],
            },
            Expr::Seq(items) => items.iter().fold(vec![at], |from, item| {
                from.into_iter()
                    .flat_map(|p| self.ends(item, input, p))
                    .collect()
            }),
            Expr::Alt(items) => items.iter().flat_map(|i| self.ends(i, input, at)).collect(),
            Expr::Opt(e) => {
                let mut v = vec![at];
                v.extend(self.ends(e, input, at));
                v
            }
            Expr::Star(e) => {
                let mut seen = vec![at];
                let mut frontier = vec![at];
                while !frontier.is_empty() {
                    let next: Vec<usize> = frontier
                        .iter()
                        .flat_map(|&p| self.ends(e, input, p))
                        .filter(|p| !seen.contains(p))
                        .collect();
                    seen.extend(&next);
                    frontier = next;
                }
                seen
            }
        };
        out.sort_unstable();
        out.dedup();
        out
    }

    /// A sentence of the grammar, chosen by `seed`: what a decoder held to it could write.
    #[must_use]
    pub fn sample(&self, seed: u64) -> String {
        let mut state = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407)
            | 1;
        let mut next = move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % (n.max(1) as u64)).unwrap_or(0)
        };
        let mut out = String::new();
        self.write(self.get(&self.rules[0].0), &mut next, &mut out);
        out
    }

    fn write(&self, e: &Expr, next: &mut impl FnMut(usize) -> usize, out: &mut String) {
        // Characters a negated class is sampled from: ordinary text, and what an IRI holds.
        const ORDINARY: &str = "abcxyz019 /:.-_#?=";
        match e {
            Expr::Text(s) => out.push_str(s),
            Expr::Rule(r) => self.write(self.get(r), next, out),
            Expr::Class {
                ranges,
                negated: false,
            } => {
                let (a, b) = ranges[next(ranges.len())];
                let span = (b as u32) - (a as u32) + 1;
                out.push(
                    char::from_u32(a as u32 + u32::try_from(next(span as usize)).unwrap_or(0))
                        .unwrap_or(a),
                );
            }
            Expr::Class {
                ranges,
                negated: true,
            } => {
                let allowed: Vec<char> = ORDINARY
                    .chars()
                    .filter(|c| !ranges.iter().any(|(a, b)| (a..=b).contains(&c)))
                    .collect();
                out.push(allowed[next(allowed.len())]);
            }
            Expr::Seq(items) => {
                for i in items {
                    self.write(i, next, out);
                }
            }
            Expr::Alt(items) => self.write(&items[next(items.len())], next, out),
            Expr::Opt(e) => {
                if next(2) == 1 {
                    self.write(e, next, out);
                }
            }
            Expr::Star(e) => {
                for _ in 0..next(3) {
                    self.write(e, next, out);
                }
            }
        }
    }
}
