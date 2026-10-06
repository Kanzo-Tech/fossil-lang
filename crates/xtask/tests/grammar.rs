//! Every production `grammar.bnf` defines is derivable from `Program`.
//!
//! The file is normative and ahead of the parser on purpose, but a production
//! no derivation reaches is a rule about nothing, implemented or not.

use std::collections::{BTreeMap, BTreeSet};

fn grammar() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../grammar.bnf");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} unreadable: {e}", path.display()))
}

/// The banner heading of a `═══ NAME ═══` line, cut at the parenthetical.
fn banner(line: &str) -> Option<String> {
    let t = line.trim();
    let t = t.strip_prefix("(*")?.strip_suffix("*)")?.trim();
    let t = t.trim_matches(|c| c == '═' || c == ' ');
    if t.is_empty() || !t.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    // A banner is `═══ NAME ═══`; the line must actually have carried the rule.
    line.contains('═')
        .then(|| t.split(['(', '—']).next().unwrap_or(t).trim().to_string())
}

fn identifiers_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if !word.is_empty() && word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            out.push(word.to_string());
        }
    }
    out
}

/// The productions of the `GRAMMAR` half of the file, each with the names its right-hand side
/// refers to. The lexical layer is excluded: its terminals are defined for the lexer and most are
/// reached by no production at all.
fn grammar_rules(src: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut rules: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut in_grammar = false;
    let mut depth = 0usize;
    for line in src.lines() {
        if let Some(name) = banner(line) {
            in_grammar = name == "GRAMMAR";
            current = None;
            continue;
        }
        let opens = line.matches("(*").count();
        let closes = line.matches("*)").count();
        let was_open = depth > 0;
        depth = depth + opens - closes.min(depth + opens);
        if !in_grammar || was_open || opens > 0 {
            if opens > 0 || was_open {
                current = None;
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let (head, rhs) = match line.split_once(":=") {
            Some((lhs, rhs)) if !line.starts_with([' ', '\t']) => {
                let name = lhs.trim().to_string();
                (Some(name), rhs)
            }
            _ if line.starts_with([' ', '\t']) => (None, line),
            _ => continue,
        };
        if let Some(name) = head {
            current = Some(name.clone());
            rules.entry(name).or_default();
        }
        if let Some(name) = current.clone() {
            let refs = identifiers_in(rhs);
            rules.entry(name).or_default().extend(refs);
        }
    }
    rules
}

#[test]
fn every_production_is_reachable_from_program() {
    let rules = grammar_rules(&grammar());
    let mut seen = BTreeSet::new();
    let mut stack = vec!["Program".to_string()];
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(refs) = rules.get(&name) {
            stack.extend(refs.iter().cloned());
        }
    }
    let mut orphans: Vec<&String> = rules.keys().filter(|n| !seen.contains(*n)).collect();
    orphans.sort();

    assert!(
        orphans.is_empty(),
        "grammar.bnf defines {orphans:?} and no derivation from Program reaches them.\n\
         A production the file is ahead of the parser on is still a production: it has to be \
         DERIVABLE, or the file specifies a form no program can contain. Add it to the \
         alternation that should offer it.",
    );
}

#[test]
fn the_reachability_guard_read_the_grammar() {
    let rules = grammar_rules(&grammar());
    assert!(
        rules.len() > 25,
        "only {} productions parsed: {:?}",
        rules.len(),
        rules.keys()
    );
    assert!(rules.contains_key("Program") && rules.contains_key("PostfixOp"));
    assert!(
        rules["TopLevel"].contains("Mapping"),
        "TopLevel's alternation no longer parses: {:?}",
        rules["TopLevel"],
    );
    assert!(
        !rules.contains_key("STRING") && !rules.contains_key("KEYWORD"),
        "the lexical layer leaked into the reachability graph, where most terminals are orphans",
    );
}
