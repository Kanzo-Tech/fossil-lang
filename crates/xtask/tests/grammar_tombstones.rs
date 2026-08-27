//! `grammar.bnf` says what the language IS; it also says what the language is NOT, and only
//! the first half had a check.
//!
//! The file is normative and DELIBERATELY AHEAD of the parser: a production written there and
//! absent from `crates/fossil-syntax` is work outstanding, not an error in the file. That
//! asymmetry is the design and this test does not touch it — a name the file DEFINES is exempt
//! from the tombstone reader however far behind the parser is, and
//! `the_tombstone_guard_is_reading_both_trees` derives that exemption rather than naming an
//! instance of it. It named one until `PolicyDef` landed in the parser, at which point the pin
//! was asserting a property of a production that no longer had it.
//!
//! The asymmetry is safe in one direction only. Deleting a tombstone and adding the production
//! puts the parser's own tests red, so that direction announces itself. **A tombstone written
//! for a form the parser still constructs is silent** — a normative document asserting the
//! absence of something that exists, with CI green. The file carries upwards of thirty such
//! assertions («There is no PIPE», «`iri` was a keyword and is gone», the whole NOT IN MVP
//! list) and until this test nothing compared one of them against the tree.
//!
//! # What is derived, and why nothing here is a list
//!
//! `engine_reach.rs` derives the engine linkers from `cargo metadata`, `tokio_placement.rs`
//! derives the real tokio table from the manifests, `cargo xtask catalogue --check` proves the
//! generated files current. A hand-written list of tombstones would be the fourth thing in this
//! repository to drift, and it would drift the way the two `-p …` lists did. So:
//!
//! - The **tombstones** come out of `grammar.bnf`'s own `(* … *)` commentary, by PHRASE — «there
//!   is no X», «X is gone», «there used to be a rule N» — plus every entry of the `NOT IN MVP`
//!   section, which is a tombstone list by its own banner («productions intentionally absent»).
//!   A new tombstone phrased like the existing ones is picked up with no edit here.
//! - The **live forms** come out of `crates/fossil-syntax`: the `SyntaxKind` variants the tree
//!   is built from, the `Token` variants the lexer emits, and the `parse_*` functions the parser
//!   is made of. Each is read from its definition site, so a rename moves the check with it.
//! - The **reserved words** come out of the `RESERVED KEYWORDS` section of the file and out of
//!   the lexer's `#[token("…")]` attributes, and the two must agree in BOTH directions.
//!
//! # The four claims
//!
//! 1. `tombstoned_forms_are_absent_from_the_parser` — no name the file declares absent is a
//!    `SyntaxKind`, a lexer `Token` or a parser production. This is the defect above.
//! 2. `every_lexeme_the_lexer_claims_is_written_in_the_grammar` — the reverse direction, and the
//!    one the asymmetry does NOT excuse: the grammar may run ahead of the parser, the parser may
//!    never run ahead of the grammar. A byte the lexer gives a meaning that the file never
//!    spells is an undocumented language.
//! 3. `the_lexer_reserves_the_words_the_grammar_reserves` — both ways. A word stolen from the
//!    identifier space that `RESERVED KEYWORDS` does not name costs every program that wanted it
//!    as a column name, silently; a word the section calls NOT a keyword and the lexer claims
//!    anyway undoes in the lexer the rule the grammar was rewritten around.
//! 4. `every_production_is_reachable_from_program` — internal, and cheap: a production the file
//!    defines that no derivation from `Program` reaches is a rule about nothing. This is not the
//!    ahead-ness — an unimplemented production still has to be DERIVABLE.
//!
//! # What this CANNOT prove, and the residue is real
//!
//! **Direction 2 at the NODE level is not built, and it is not a matter of effort.** The clean
//! statement would be «every node kind the parser builds is a production the grammar names», and
//! it is false by design rather than by drift: `kind.rs` says so in its own header — *every
//! variant here is a kind the lexer or the parser actually produces, which is NOT the same as a
//! terminal of `grammar.bnf`*. Five kinds are CST shape rather than language form —
//! `BINARY_EXPR` (one node for five precedence levels), `PAREN_EXPR`, `EXPR`,
//! `INTERP_STRING_EXPR`, and the `ERROR` sentinel — and two parser functions are Pratt-loop
//! helpers rather than productions (`parse_unary_or_primary`, `parse_interpolation_body`).
//! Checking that direction needs a name mapping that neither file supplies, which is to say a
//! hand-maintained list, which is the thing this file exists to avoid. The LEXICAL half of the
//! same direction is built (claim 2) because the lexical layer is where the grammar claims
//! totality and the lexer agrees in writing: *one `Token` variant per terminal of the lexical
//! layer, and now with no exception*.
//!
//! It also cannot prove that a tombstone is TRUE of anything below the parser: a form the HIR
//! still lowers, with no token and no node, is invisible here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask sits two levels below the root")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} unreadable: {e}", path.display()))
}

// ─── Naming ──────────────────────────────────────────────────────────────────────────────────
//
// One name is written three ways across the two trees: `PipelineExpr` in the grammar,
// `PIPELINE_EXPR` as a kind, `Pipe`/`parse_pipeline` in the parser. Comparison is on a canonical
// form — underscores dropped, upper-cased, and one trailing `EXPR` or `DECL` removed, since the
// suffix is a CST convention and never part of what the grammar calls the form. `EXPR` itself is
// left alone: stripping it would leave nothing to compare.
fn canon(name: &str) -> String {
    let mut s: String = name
        .chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_uppercase)
        .collect();
    for suffix in ["EXPR", "DECL"] {
        if s.len() > suffix.len() && s.ends_with(suffix) {
            s.truncate(s.len() - suffix.len());
            break;
        }
    }
    s
}

/// `parse_source_def` → `SourceDef`; `SOURCE_DEF` → `SourceDef` in effect, since both canonicalise
/// the same. Used only to turn a parser function name into something [`canon`] can compare.
fn from_snake(snake: &str) -> String {
    snake
        .split('_')
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(char::to_uppercase)
                .into_iter()
                .flatten()
                .collect::<String>()
                + c.as_str()
        })
        .collect()
}

/// A name that could be a grammar form, as against an English word a comment happened to shout.
///
/// Three shapes, and the third is bounded on purpose: `PIPE` and `TEMPLATE` are tombstones and
/// have no underscore and no camel hump, so all-caps has to be admitted — but `MVP` and `RDF` are
/// not forms, and neither is a two-letter word. Over-admitting is cheap here and under-admitting
/// is not: a name that is not a form collides with nothing live, while a form that is not
/// admitted is a tombstone nobody checks.
fn is_form_shaped(name: &str) -> bool {
    let uppers = name.chars().filter(char::is_ascii_uppercase).count();
    let lowers = name.chars().filter(char::is_ascii_lowercase).count();
    if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
        return false;
    }
    if name.contains('_') {
        return lowers == 0 && name.len() >= 3;
    }
    if lowers > 0 {
        return uppers >= 2;
    }
    name.len() >= 4
}

fn names_in(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| is_form_shaped(w))
        .map(str::to_string)
        .collect()
}

/// Every identifier, shape unfiltered. The reachability graph wants this and the tombstone
/// reader must not have it: `Mapping` and `Rename` are one-hump names that [`is_form_shaped`]
/// refuses on purpose, and refusing them is right when the haystack is English prose and wrong
/// when it is the right-hand side of a production.
fn identifiers_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if !word.is_empty() && word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            out.push(word.to_string());
        }
    }
    out
}

// ─── grammar.bnf ─────────────────────────────────────────────────────────────────────────────

/// One `(* … *)` comment, its text flattened, paired with the banner section it sits under.
struct Comment {
    section: String,
    text: String,
}

/// The banner heading, cut at the parenthetical, exactly as `apps/docs/content.test.ts` cuts it
/// for a section citation. The two extractors agree by construction because the file
/// writes the banner one way.
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

fn comments(src: &str) -> Vec<Comment> {
    let mut out = Vec::new();
    let mut section = String::from("(preamble)");
    let mut open: Option<String> = None;
    for line in src.lines() {
        if let Some(name) = banner(line) {
            section = name;
            continue;
        }
        let mut rest = line;
        loop {
            match &mut open {
                None => match rest.find("(*") {
                    Some(i) => {
                        open = Some(String::new());
                        rest = &rest[i + 2..];
                    }
                    None => break,
                },
                Some(buf) => match rest.find("*)") {
                    Some(i) => {
                        buf.push_str(&rest[..i]);
                        out.push(Comment {
                            section: section.clone(),
                            text: buf.split_whitespace().collect::<Vec<_>>().join(" "),
                        });
                        open = None;
                        rest = &rest[i + 2..];
                    }
                    None => {
                        buf.push_str(rest);
                        buf.push(' ');
                        break;
                    }
                },
            }
        }
    }
    out
}

/// Every name the file DEFINES — the left of a `:=`, terminals included, and a comma list on the
/// left defines both (`LPAREN, RPAREN := '(' ')'`). The same rule `apps/docs/content.test.ts`
/// uses to resolve a citation.
fn defined(src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let Some((lhs, _)) = line.split_once(":=") else {
            continue;
        };
        if line.starts_with([' ', '\t', '(']) || lhs.trim().is_empty() {
            continue;
        }
        if lhs.split(',').all(|n| {
            !n.trim().is_empty()
                && n.trim()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        }) {
            out.extend(lhs.split(',').map(|n| n.trim().to_string()));
        }
    }
    out
}

/// The phrases the file uses to say a form does not exist. PHRASES, not names: a tombstone
/// written tomorrow in any of these shapes is checked with no edit here.
const ABSENCE: &[&str] = &[
    "There is no",
    "there is no",
    "There are no",
    "there are no",
    "There was a",
    "There was an",
    "There used to be",
    "is gone",
    "are gone",
    "is dead",
    "went with",
    "died with",
    "is not a token",
    "are not tokens",
    "no longer",
    "nothing ever produced",
];

/// A section whose every entry is a tombstone, by its own banner: «productions intentionally
/// absent».
const TOMBSTONE_SECTION: &str = "NOT IN MVP";

/// The one section a tombstone phrase must NOT be read out of. An `OPEN` item is a form the
/// language NEEDS and has not decided — the opposite of a tombstone — and it discusses the live
/// productions it would change.
const OPEN_SECTION: &str = "OPEN";

/// Every form `grammar.bnf` asserts does not exist, with the sentence that asserts it.
fn tombstones(src: &str) -> BTreeMap<String, String> {
    let defined_canon: BTreeSet<String> = defined(src).iter().map(|n| canon(n)).collect();
    let mut out = BTreeMap::new();
    for c in comments(src) {
        if c.section.starts_with(OPEN_SECTION) {
            continue;
        }
        // The unit of a tombstone is the COMMENT, not the sentence. «There is no annotation
        // block» and «Nothing below the parser ever read an ANNOTATION_BLOCK» are two sentences
        // of one burial, and the name is in the sentence with no marker in it. So one marker
        // anywhere in the comment puts the whole comment in scope; the sentence is kept only to
        // quote back the line that made the claim.
        let is_tombstone =
            c.section == TOMBSTONE_SECTION || ABSENCE.iter().any(|m| c.text.contains(m));
        if !is_tombstone {
            continue;
        }
        for sentence in c.text.split(". ") {
            for name in names_in(sentence) {
                if defined_canon.contains(&canon(&name)) {
                    continue;
                }
                out.entry(name)
                    .or_insert_with(|| sentence.trim().to_string());
            }
        }
    }
    out
}

// ─── crates/fossil-syntax ────────────────────────────────────────────────────────────────────

/// The variants declared by `enum <name> {`, read at the declaration site. A `//` tombstone
/// comment inside the body is not a variant and is not read as one — which is the whole point,
/// since `kind.rs` keeps its tombstones exactly there.
fn enum_variants(src: &str, decl: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in src.lines() {
        let t = line.trim();
        if !inside {
            inside = t.starts_with(decl);
            continue;
        }
        if t == "}" {
            break;
        }
        if t.starts_with("//") || t.starts_with('#') || t.starts_with("__") {
            continue;
        }
        let Some(name) = t.strip_suffix(',') else {
            continue;
        };
        let name = name.split('=').next().unwrap_or(name).trim();
        if !name.is_empty()
            && name.starts_with(|c: char| c.is_ascii_uppercase())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            out.insert(name.to_string());
        }
    }
    out
}

/// `Token::Colon => SyntaxKind::SHAPE_SEP` — the bridge, and it is exhaustive because rustc makes
/// it so. Used to cross-check the two enum readings above rather than to trust either alone.
fn token_to_kind_arms(src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let Some((lhs, rhs)) = line.split_once("=>") else {
            continue;
        };
        if !rhs.contains("SyntaxKind::") {
            continue;
        }
        for part in lhs.split('|') {
            if let Some(v) = part.trim().strip_prefix("Token::") {
                out.insert(v.trim().to_string());
            }
        }
    }
    out
}

/// Every literal the lexer claims, paired with nothing: `#[token("from")]` → `from`.
fn lexer_literals(src: &str) -> BTreeSet<String> {
    src.lines()
        .filter_map(|l| {
            let t = l.trim().strip_prefix("#[token(\"")?;
            Some(t.split('"').next()?.to_string())
        })
        .collect()
}

fn parse_functions(files: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for f in files {
        for line in read(f).lines() {
            let t = line.trim_start();
            let stem = ["fn ", "pub fn ", "pub(crate) fn "]
                .into_iter()
                .find_map(|p| t.strip_prefix(p))
                .and_then(|rest| rest.split('(').next())
                .and_then(|name| name.trim().strip_prefix("parse_"));
            if let Some(stem) = stem {
                out.insert(stem.to_string());
            }
        }
    }
    out
}

const PARSER_FILES: &[&str] = &[
    "crates/fossil-syntax/src/parser/mod.rs",
    "crates/fossil-syntax/src/parser/items.rs",
    "crates/fossil-syntax/src/parser/expr.rs",
];

/// The `ERROR` and `EOF` sentinels are excluded, and this is the one exclusion in the file.
/// Neither is a form of the language: `ERROR` is what a byte with no rule becomes and `EOF` is
/// the end of the token stream, and both spellings are ordinary English the grammar's prose uses
/// for other purposes («are an ERROR naming both IRIs»). A tombstone cannot name either, because
/// there is nothing to bury.
const SENTINELS: &[&str] = &["ERROR", "EOF"];

/// Every form `crates/fossil-syntax` actually builds, canonicalised.
fn live_forms() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for k in enum_variants(
        &read("crates/fossil-syntax/src/kind.rs"),
        "pub enum SyntaxKind",
    ) {
        if SENTINELS.contains(&k.as_str()) {
            continue;
        }
        out.insert(canon(&k), format!("SyntaxKind::{k}"));
    }
    for t in enum_variants(&read("crates/fossil-syntax/src/lexer.rs"), "pub enum Token") {
        out.entry(canon(&t))
            .or_insert_with(|| format!("lexer::Token::{t}"));
    }
    for p in parse_functions(PARSER_FILES) {
        out.entry(canon(&from_snake(&p)))
            .or_insert_with(|| format!("parser::parse_{p}"));
    }
    out
}

// ─── 1. The tombstones ───────────────────────────────────────────────────────────────────────

#[test]
fn tombstoned_forms_are_absent_from_the_parser() {
    let src = read("grammar.bnf");
    let live = live_forms();

    let mut lies: Vec<String> = tombstones(&src)
        .into_iter()
        .filter_map(|(name, sentence)| {
            let built = live.get(&canon(&name))?;
            Some(format!(
                "  {name} — built as {built}\n    grammar.bnf says: «{sentence}»"
            ))
        })
        .collect();
    lies.sort();

    assert!(
        lies.is_empty(),
        "grammar.bnf declares these forms ABSENT and crates/fossil-syntax builds them:\n{}\n\n\
         The file is normative, so one of the two is a lie. The tree wins: either the parser \
         still constructs a retired form and must stop, or the form came back and the tombstone \
         must go with it. Deleting the tombstone alone is not the fix — a form that exists needs \
         a production.",
        lies.join("\n"),
    );
}

/// A guard over an empty set has stopped guarding. Four ways this one could go vacuous — no
/// comments parsed, no tombstones matched, no live forms read, or the canonical form stopped
/// collapsing the three spellings of one name — and each is pinned.
#[test]
fn the_tombstone_guard_is_reading_both_trees() {
    let src = read("grammar.bnf");
    assert!(src.contains(":="), "grammar.bnf is not the grammar");

    let comments = comments(&src);
    assert!(
        comments.len() > 40,
        "only {} comments parsed out of grammar.bnf",
        comments.len()
    );
    assert!(
        comments.iter().any(|c| c.section == TOMBSTONE_SECTION),
        "the {TOMBSTONE_SECTION} banner no longer parses, so its entries are read as prose",
    );
    assert!(
        comments.iter().any(|c| c.section.starts_with(OPEN_SECTION)),
        "the {OPEN_SECTION} banner no longer parses, so undecided forms are read as tombstones",
    );

    let found = tombstones(&src);
    assert!(
        found.len() > 20,
        "only {} tombstones extracted: {found:?}",
        found.len()
    );
    for expected in [
        "PIPE",               // «There is no PIPE» — the flagship, lexical layer
        "TEMPLATE",           // the backtick literal
        "ABS_IRI",            // `<http://…>`
        "PrefixDecl",         // the vocabulary declaration
        "PipelineExpr",       // the node `a |> f()` built
        "FieldRef",           // a leading `.`
        "IRIExpr",            // the CURIE, the absolute IRI and the template as one node
        "ANNOTATION_BLOCK",   // `p = e { a = v }`
        "T_COLON",            // a terminal the lexer never emitted
        "TypeAnnotation",     // NOT IN MVP
        "ExportedDefinition", // NOT IN MVP — `@export`
        "LambdaExpr",         // NOT IN MVP
    ] {
        assert!(
            found.contains_key(expected),
            "the extractor stopped finding the {expected} tombstone"
        );
    }

    // The ahead-ness is the design, and it is a property of the READER rather than of any one
    // production: a name the file DEFINES is skipped by `tombstones`, whether the parser builds
    // it or not. Derived over every definition, so a production written tomorrow is covered with
    // no edit here — the pin used to name `PolicyDef` as the live instance, and stopped being
    // about ahead-ness the moment `PolicyDef` was parsed.
    let defined_canon: BTreeSet<String> = defined(&src).iter().map(|n| canon(n)).collect();
    for name in found.keys() {
        assert!(
            !defined_canon.contains(&canon(name)),
            "`{name}` is a production grammar.bnf DEFINES and it was read as a tombstone; a file \
             that is ahead of the parser would now fail this suite for being ahead",
        );
    }

    // `PolicyDef` keeps a pin of its own, with its direction reversed by the landing. The parser
    // builds `POLICY_DEF`, so the file has to keep defining `PolicyDef` — which is the ONE
    // node-level instance of direction 2 (see this file's header: the general form needs a name
    // mapping neither file supplies, and this one name maps by `canon` alone).
    assert!(
        defined(&src).contains("PolicyDef"),
        "crates/fossil-syntax builds POLICY_DEF and grammar.bnf stopped defining PolicyDef — \
         the parser may never run ahead of the grammar",
    );

    let live = live_forms();
    assert!(
        live.len() > 60,
        "only {} live forms read out of fossil-syntax",
        live.len()
    );
    for (form, spelling) in [
        ("PIPELINE", "PipelineExpr"),
        ("POSTFIX", "PostfixExpr"),
        ("SOURCEDEF", "SourceDef"),
    ] {
        assert_eq!(
            canon(spelling),
            form,
            "canon() stopped collapsing {spelling}"
        );
    }
    for known in ["TYPEDEF", "MAPPINGHEADER", "ATATTR", "INTERPOLATION"] {
        assert!(
            live.contains_key(known),
            "the live-form reader lost {known}"
        );
    }
    for sentinel in SENTINELS {
        assert!(
            !live.contains_key(*sentinel),
            "{sentinel} is excluded and is being read anyway"
        );
    }
}

/// `kind.rs` declares its variants twice — once in the enum, once in `from_raw_value`'s arms —
/// and `syntax_kind_round_trip_for_all_variants` proves the second is right at runtime. Reading
/// both and comparing is how this file proves its own enum reader, without a list of variants.
/// The same trick over `Token`: the lexer declares them, `indent::token_to_kind` matches them
/// exhaustively, and rustc guarantees the second is total.
#[test]
fn the_enum_readers_agree_with_a_second_declaration_of_the_same_set() {
    let kinds = read("crates/fossil-syntax/src/kind.rs");
    let declared = enum_variants(&kinds, "pub enum SyntaxKind");
    let arms: BTreeSet<String> = kinds
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_suffix(',')?
                .split_once("=> Self::")
                .map(|(_, v)| v.trim().to_string())
        })
        .collect();
    assert!(!arms.is_empty(), "from_raw_value's arms no longer parse");
    assert_eq!(
        declared, arms,
        "the SyntaxKind reader disagrees with from_raw_value's own arms"
    );

    let tokens = enum_variants(&read("crates/fossil-syntax/src/lexer.rs"), "pub enum Token");
    let bridged = token_to_kind_arms(&read("crates/fossil-syntax/src/indent.rs"));
    assert!(
        !bridged.is_empty(),
        "indent::token_to_kind no longer parses"
    );
    assert_eq!(
        tokens, bridged,
        "the Token reader disagrees with token_to_kind's exhaustive match"
    );
}

// ─── 2. The parser is never ahead of the grammar ─────────────────────────────────────────────

#[test]
fn every_lexeme_the_lexer_claims_is_written_in_the_grammar() {
    let src = read("grammar.bnf");
    let mut undocumented: Vec<String> = lexer_literals(&read("crates/fossil-syntax/src/lexer.rs"))
        .into_iter()
        .filter(|lit| !src.contains(&format!("'{lit}'")))
        .collect();
    undocumented.sort();

    assert!(
        undocumented.is_empty(),
        "the lexer gives these lexemes a meaning and grammar.bnf never spells them: {undocumented:?}\n\
         The grammar may run ahead of the parser; the parser may never run ahead of the grammar. \
         A token the file does not name is an undocumented language.",
    );
}

#[test]
fn the_lexeme_guard_reads_the_lexer() {
    let lits = lexer_literals(&read("crates/fossil-syntax/src/lexer.rs"));
    assert!(
        lits.len() > 20,
        "only {} #[token(\"…\")] literals read: {lits:?}",
        lits.len()
    );
    for expected in ["from", "true", ":=", "?", "%"] {
        assert!(
            lits.contains(expected),
            "the lexer literal reader lost {expected:?}"
        );
    }
    assert!(
        !lits.contains("|>"),
        "`|>` is a token again — grammar.bnf, § LEXICAL LAYER says it is not one",
    );
}

// ─── 3. The reserved words ───────────────────────────────────────────────────────────────────

/// `RESERVED KEYWORDS (cannot be identifiers)`, read out of the section that says so: the first
/// content line is the reserved list, and the sentence beginning `NOT KEYWORDS` names, in
/// backticks, the words that must stay ordinary identifiers.
fn reserved_sections(src: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in src.lines() {
        if let Some(name) = banner(line) {
            inside = name == "RESERVED KEYWORDS";
            continue;
        }
        if inside {
            let t = line.trim();
            if let Some(t) = t.strip_prefix("(*").and_then(|t| t.strip_suffix("*)")) {
                lines.push(t.trim().to_string());
            }
        }
    }
    let reserved: BTreeSet<String> = lines
        .iter()
        .find(|l| !l.is_empty())
        .map(|l| l.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();

    let joined = lines.join(" ");
    let not_keywords = joined
        .split_once("NOT KEYWORDS")
        .map(|(_, rest)| {
            let claim = rest.split_once(". Every").map_or(rest, |(c, _)| c);
            claim
                .split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect::<BTreeSet<String>>()
        })
        .unwrap_or_default();
    (reserved, not_keywords)
}

/// A word-shaped lexeme is one that would otherwise be an `IDENT`; a lexer rule for it takes the
/// word out of the identifier space, whatever the rule calls itself.
fn word_shaped(lit: &str) -> bool {
    !lit.is_empty() && lit.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
}

#[test]
fn the_lexer_reserves_the_words_the_grammar_reserves() {
    let src = read("grammar.bnf");
    let (reserved, not_keywords) = reserved_sections(&src);
    let claimed: BTreeSet<String> = lexer_literals(&read("crates/fossil-syntax/src/lexer.rs"))
        .into_iter()
        .filter(|l| word_shaped(l))
        .collect();

    let unlisted: Vec<&String> = claimed.difference(&reserved).collect();
    assert!(
        unlisted.is_empty(),
        "the lexer takes {unlisted:?} out of the identifier space and grammar.bnf, \
         § RESERVED KEYWORDS does not list them.\n\
         The section's title is «cannot be identifiers» and that is exactly what a #[token(\"…\")] \
         on a word does. A word reserved without being written down costs every program that \
         wanted it as a column name, and costs it silently.",
    );

    let stolen: Vec<&String> = claimed.intersection(&not_keywords).collect();
    assert!(
        stolen.is_empty(),
        "the lexer claims {stolen:?}, which grammar.bnf, § RESERVED KEYWORDS names as NOT \
         keywords.\n\
         Reserving one undoes in the lexer the rule the grammar was rewritten around — the verbs \
         stopped being grammar, and a keyword is grammar.",
    );

    let missing: Vec<&String> = reserved.difference(&claimed).collect();
    assert!(
        missing.is_empty(),
        "grammar.bnf reserves {missing:?} and the lexer has no rule for them, so they lex as \
         IDENT and a program may still use them as column names. The file promises a refusal the \
         parser does not make.",
    );
}

#[test]
fn the_reserved_word_guard_read_the_section() {
    let (reserved, not_keywords) = reserved_sections(&read("grammar.bnf"));
    assert!(
        reserved.len() >= 6 && reserved.contains("from") && reserved.contains("false"),
        "the reserved list no longer parses: {reserved:?}",
    );
    assert!(
        not_keywords.len() >= 8 && not_keywords.contains("where") && not_keywords.contains("join"),
        "the NOT KEYWORDS list no longer parses: {not_keywords:?}",
    );
    assert!(
        reserved.is_disjoint(&not_keywords),
        "grammar.bnf reserves a word it also calls not a keyword: {:?}",
        reserved.intersection(&not_keywords).collect::<Vec<_>>(),
    );
    assert!(
        word_shaped("null") && !word_shaped(":="),
        "word_shaped stopped telling the two apart"
    );
}

// ─── 4. The file against itself ──────────────────────────────────────────────────────────────

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
    let rules = grammar_rules(&read("grammar.bnf"));
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
    let rules = grammar_rules(&read("grammar.bnf"));
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
