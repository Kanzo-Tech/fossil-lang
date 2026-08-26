//! `catalogue.bnf` → the provider statics and the stdlib catalogue.
//!
//! # Why this exists
//!
//! `catalogue.bnf` says WHAT NAMES EXIST — the name written after `io.`, the
//! extensions the row accepts, which capabilities it declares, and the
//! signature and lowering of every stdlib function. The Rust is GENERATED from
//! it rather than checked against it, because a generated file writes
//! `decodes decode_shex` and `op Where` as Rust PATHS the compiler resolves: a
//! row naming a decoder or an operator nobody wrote is a build failure, where a
//! parity test could only compare strings and a `fn` pointer has no name at run
//! time. Everything a row implies comes off its tokens for the same reason —
//! `NativeReader`'s variant name and its `table_function` are both derived from
//! `native <fn>`, and a stdlib row's receiver and member are derived from its
//! dotted name by `fossil_hir::stdlib::split_receiver`.
//!
//! The reference is rust-analyzer's `rust.ungram`, which `catalogue.bnf` already
//! cites: a data file beside the compiler, a generator, and a checked-in output
//! that a `--check` mode proves current.
//!
//! # Where a row goes, and why that is derivable
//!
//! `catalogue.bnf` draws the DATA/BEHAVIOUR line itself: a row's data lives in
//! the file, and the function that turns bytes into rows or into types is
//! supplied by the crate that can link it. So a row that `decodes` is emitted
//! into `fossil-descriptors-output`, which links the shape-language parsers, and
//! every other row into `fossil-base`, which the compiler always has. That is
//! also why `DATA` — the default of `System::providers` — is exactly the
//! `fossil-base` half: a host that links no schema language still gets every row
//! it can execute.
//!
//! # The lexer, and why it is not a `split('"')`
//!
//! This module used to extract a row's string literals with
//! `body.split('"').skip(1).step_by(2)` — "the odd segments are literals" —
//! which is exactly true for a file whose only literals are extensions like
//! `"csv"` and exactly false for the stdlib half. A template is SQL text: it
//! carries `|`, `%`, `$`, `{`, `}`, backslashes, doubled single quotes and, in
//! `str.strip_html`, double quotes of its own. One `"` inside one literal and
//! every later segment of the line changes parity, so the failure is not a
//! parse error but a silently different catalogue.
//!
//! [`lex`] is therefore a real scanner: it knows a string literal from a
//! comment from a word, and `\"` and `\\` are escapes inside a literal. That
//! also makes a row free to span lines, which four of the templates did as Rust
//! constants and which a line scanner could not have read.
//!
//! # What this does NOT generate, deliberately
//!
//! The ARGUMENT. `catalogue.bnf` keeps it in the `(* … *)` commentary, and the
//! doc comments emitted here are the one-line derived kind — what the row is,
//! not why it is that. Prose that reasons belongs next to the reasoning, and a
//! generator that tried to carry it would make the `.bnf` a second Rust file
//! with different syntax.
//!
//! That is also where the measured templates' prose went. `SLUG_TEMPLATE` and
//! `STRIP_HTML_TEMPLATE` were Rust constants carrying paragraphs of fuzzing
//! results, and the value they justify is now a literal in the file — so the
//! paragraphs are `(* … *)` commentary above the row they are about, which is
//! the placement `catalogue.bnf` already uses for every argument it makes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ── The lexer ──────────────────────────────────────────────────────────────

/// One token of `catalogue.bnf`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// An identifier, possibly dotted: `csv`, `read_csv_auto`, `str.replace`,
    /// `String`. A `.` is part of a word only when a word character follows it,
    /// which is what tells `str.replace` from the `.` that ends a row.
    Word(String),
    /// A `"…"` literal, unescaped. `\"` and `\\` are the two escapes.
    Str(String),
    /// Punctuation, including the two-character `->`.
    Punct(&'static str),
}

/// The punctuation `catalogue.bnf` uses, longest first so `->` beats `-`.
const PUNCT: &[&str] = &["->", "=", ";", ".", "(", ")", ",", ":", "+", "?"];

/// Tokenise the whole file, skipping `(* … *)` commentary.
///
/// Comment detection takes precedence over string detection, which is what lets
/// the commentary contain quotation marks (it is full of them) without the
/// scanner mistaking one for the start of a literal.
///
/// # Panics
///
/// On an unterminated string literal, and on any character that begins no
/// token — both are malformed input to a file that is now a source of truth.
fn lex(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // Commentary first: `(*` opens it and only `*)` closes it, so a `"` in
        // between is prose and not a literal.
        if chars[i] == '(' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&')')) {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '"' {
            i += 1;
            let mut s = String::new();
            loop {
                let c = *chars
                    .get(i)
                    .unwrap_or_else(|| panic!("catalogue.bnf: unterminated string literal"));
                match c {
                    '"' => {
                        i += 1;
                        break;
                    }
                    '\\' => {
                        let n = *chars.get(i + 1).unwrap_or_else(|| {
                            panic!("catalogue.bnf: a `\\` at the end of the file")
                        });
                        // Only the two escapes a literal needs to be closed and
                        // to contain a backslash. Anything else is itself, so a
                        // regex's `\p{L}` survives being written literally.
                        match n {
                            '"' | '\\' => s.push(n),
                            other => {
                                s.push('\\');
                                s.push(other);
                            }
                        }
                        i += 2;
                    }
                    other => {
                        s.push(other);
                        i += 1;
                    }
                }
            }
            toks.push(Tok::Str(s));
            continue;
        }
        if chars[i].is_ascii_alphanumeric() || chars[i] == '_' {
            let mut w = String::new();
            while i < chars.len() {
                let c = chars[i];
                if c.is_ascii_alphanumeric() || c == '_' {
                    w.push(c);
                    i += 1;
                } else if c == '.'
                    && chars
                        .get(i + 1)
                        .is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_')
                {
                    w.push(c);
                    i += 1;
                } else {
                    break;
                }
            }
            toks.push(Tok::Word(w));
            continue;
        }
        let rest: String = chars[i..].iter().take(2).collect();
        let Some(p) = PUNCT.iter().find(|p| rest.starts_with(**p)) else {
            panic!("catalogue.bnf: unexpected character `{}`", chars[i]);
        };
        toks.push(Tok::Punct(p));
        i += p.chars().count();
    }
    toks
}

// ── The row model ──────────────────────────────────────────────────────────

/// How a row's bytes become rows a plan can scan.
#[derive(Debug, PartialEq, Eq)]
pub enum Reads {
    /// `reads native <fn>` — a `DuckDB` table function, named by the token.
    Native(String),
    /// `reads materialised` — something outside the reader produces the
    /// relation and the core only scans it.
    Materialised,
}

/// What a catalogue row's parameter takes, or what it gives back.
///
/// The twelve spellings `catalogue.bnf` admits, flat: seven scalars and the
/// five positions whose meaning depends on the receiver. It mirrors
/// `fossil_hir::stdlib::SigTy` — [`Self::rust_path`] is the mirror, and
/// [`Self::page_name`] is how the reference page spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigTy {
    String,
    Integer,
    Float,
    Bool,
    Date,
    DateTime,
    SeqString,
    Rows,
    Predicate,
    Column,
    Binding,
    Aggregate,
}

impl SigTy {
    /// The one spelling `catalogue.bnf` writes, or `None` for a word that names
    /// no type.
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "String" => Self::String,
            "Integer" => Self::Integer,
            "Float" => Self::Float,
            "Bool" => Self::Bool,
            "Date" => Self::Date,
            "DateTime" => Self::DateTime,
            "SeqString" => Self::SeqString,
            "Rows" => Self::Rows,
            "Predicate" => Self::Predicate,
            "Column" => Self::Column,
            "Binding" => Self::Binding,
            "Aggregate" => Self::Aggregate,
            _ => return None,
        })
    }

    /// The `fossil_hir::stdlib::SigTy` expression this spelling denotes.
    fn rust_path(self) -> &'static str {
        match self {
            Self::String => "SigTy::Scalar(ScalarTy::String)",
            Self::Integer => "SigTy::Scalar(ScalarTy::Integer)",
            Self::Float => "SigTy::Scalar(ScalarTy::Float)",
            Self::Bool => "SigTy::Scalar(ScalarTy::Bool)",
            Self::Date => "SigTy::Scalar(ScalarTy::Date)",
            Self::DateTime => "SigTy::Scalar(ScalarTy::DateTime)",
            Self::SeqString => "SigTy::Scalar(ScalarTy::SeqString)",
            Self::Rows => "SigTy::Rows",
            Self::Predicate => "SigTy::Predicate",
            Self::Column => "SigTy::Column",
            Self::Binding => "SigTy::Binding",
            Self::Aggregate => "SigTy::Aggregate",
        }
    }

    /// How the reference page spells this position. The two that differ from
    /// the file's own word are the two the type page already had names for.
    #[must_use]
    pub const fn page_name(self) -> &'static str {
        match self {
            Self::String => "String",
            Self::Integer => "Integer",
            Self::Float => "Float",
            Self::Bool => "Bool",
            Self::Date => "Date",
            Self::DateTime => "DateTime",
            Self::SeqString => "Seq<String>",
            Self::Rows => "Relation",
            Self::Predicate => "Predicate",
            Self::Column => "Column",
            Self::Binding => "Binding",
            Self::Aggregate => "Aggregate",
        }
    }
}

/// How many arguments one parameter position takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Arity {
    /// Exactly one. Written by using `:` with no suffix.
    #[default]
    One,
    /// One or more, written `name: Type+`.
    OneOrMore,
    /// Zero or one, written `name: Type?`.
    Optional,
}

impl Arity {
    fn rust_path(self) -> &'static str {
        match self {
            Self::One => "Arity::One",
            Self::OneOrMore => "Arity::OneOrMore",
            Self::Optional => "Arity::Optional",
        }
    }

    /// The reference page's suffix for this arity.
    #[must_use]
    pub const fn page_suffix(self) -> &'static str {
        match self {
            Self::One => "",
            Self::OneOrMore => "…",
            Self::Optional => "?",
        }
    }
}

/// One parameter position: what it is called, what it takes, how many, and
/// whether the call site writes its name.
///
/// `named` is spelled by the SEPARATOR — `on = Predicate` rather than
/// `on: Predicate` — because that is the separator the call site writes. A
/// reader copying a signature off the reference page copies a program that
/// parses, and the file and the page now agree because one is printed from the
/// other.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: SigTy,
    pub arity: Arity,
    pub named: bool,
}

/// A row's call shape: what it takes and what it gives back.
#[derive(Debug, Clone)]
pub struct Signature {
    pub params: Vec<Param>,
    /// What the row gives back — INDEPENDENT of the lowering. `seq.count` is an
    /// operator returning `Integer` and `io.csv` is an operator returning
    /// `Rows` with a scalar parameter, so a grammar that tied `-> Rows` to `op`
    /// could express neither.
    pub ret: SigTy,
}

/// How a row compiles.
#[derive(Debug, Clone)]
pub enum Lowering {
    /// `expr "<template>"` — a scalar SQL expression with `%N` argument holes.
    Expr(String),
    /// `op <Variant>` — one operator of the algebra, named by its `PlanOp`
    /// variant. It is emitted as a Rust path, so an `op` token naming a variant
    /// nobody wrote is a build failure, exactly as `decodes decode_shex` is.
    Op(String),
}

/// One stdlib row: `fn <dotted-name> = <signature> ; <lowering> .`
///
/// The receiver and the member are NOT here, and that is the point: both are
/// derived from the dotted name by `fossil_hir::stdlib::split_receiver`, so a
/// row cannot carry a receiver that disagrees with its own spelling. A `.bnf`
/// row that wrote one would be re-introducing the field that drifts.
#[derive(Debug, Clone)]
pub struct FnRow {
    pub name: String,
    pub sig: Signature,
    pub lowering: Lowering,
}

/// One `row … = … .` line of `catalogue.bnf` — the `io.` half.
#[derive(Debug)]
pub struct Row {
    /// What is written after `io.`.
    pub name: String,
    /// The extensions the row accepts, in file order.
    pub extensions: Vec<String>,
    /// The `read rows` capability.
    pub reads: Option<Reads>,
    /// The `read types` capability — the name of the decoding function the
    /// implementing crate supplies.
    pub decodes: Option<String>,
    /// What a program gets if it writes this constructor in a VALUE position.
    ///
    /// Optional, and its absence is a real answer rather than an omission: a
    /// row that `decodes` gives back a shape document, and `SigTy` has no
    /// spelling for one. Inventing `Rows` for `io.shex` would be the mistake
    /// `add`/`add_verb` used to make one layer down, where three constructors
    /// of relations declared that they returned a string.
    pub call: Option<(Signature, Lowering)>,
}

impl Row {
    /// The Rust identifier of this row's static: the name, upper-cased.
    fn static_ident(&self) -> String {
        self.name.to_ascii_uppercase()
    }

    /// Does this row's behaviour have to be linked by a crate that owns a
    /// shape-language parser? See the module doc.
    fn needs_a_decoder(&self) -> bool {
        self.decodes.is_some()
    }
}

/// Everything `catalogue.bnf` declares.
#[derive(Debug, Default)]
pub struct Catalogue {
    /// The `io.` rows, in file order.
    pub rows: Vec<Row>,
    /// The stdlib rows, in file order.
    pub fns: Vec<FnRow>,
}

impl Catalogue {
    /// Every row the checker's `FunctionRegistry` carries: the stdlib rows, and
    /// the `io.` rows that declared a signature.
    ///
    /// The `io.` rows are here and not stated a second time in Rust, which is
    /// the drift this fold ends: the registry used to carry `csv`, `json` and
    /// `parquet` written out by hand while `catalogue.bnf` carried six, so
    /// `io.rdf` — a real source a program may bind — had no signature at all
    /// and no diagnostic when written in a value position.
    #[must_use]
    pub fn registry_rows(&self) -> Vec<(String, &Signature, &Lowering)> {
        let mut out: Vec<(String, &Signature, &Lowering)> = self
            .fns
            .iter()
            .map(|f| (f.name.clone(), &f.sig, &f.lowering))
            .collect();
        for row in &self.rows {
            if let Some((sig, lowering)) = &row.call {
                out.push((format!("io.{}", row.name), sig, lowering));
            }
        }
        out
    }
}

/// Is `head` a NAMESPACE — a name with nothing to dispatch on — rather than a
/// type whose members these are?
///
/// The same three-way question `fossil_hir::stdlib::receiver_of` answers,
/// reduced to the one bit this crate needs: whether the reference page prints a
/// row's dotted name or its member. It is a second statement of that rule and
/// says so; `crates/xtask/tests/receiver_agrees.rs` derives the comparison from
/// `fossil_hir` itself rather than repeating the answer here.
#[must_use]
pub fn is_namespace_head(head: &str) -> bool {
    !matches!(head, "str" | "seq")
}

/// The `NativeReader` variant a `native <fn>` token names.
///
/// `read_csv_auto` → `CsvAuto`, `read_parquet` → `Parquet`: strip the `read_`
/// that every table function starts with, then upper-camel the rest. The rule is
/// here rather than in a table so that a new reader is a row in the file and
/// nothing else — which is the whole point of generating this.
fn variant_of(table_fn: &str) -> String {
    table_fn
        .strip_prefix("read_")
        .unwrap_or(table_fn)
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

// ── The parser ─────────────────────────────────────────────────────────────

/// A cursor over the token stream, so each statement can be read by the same
/// small set of expectations.
struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.peek() == Some(&Tok::Punct(
            PUNCT.iter().find(|q| **q == p).expect("a known punct"),
        )) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) {
        assert!(self.eat_punct(p), "catalogue.bnf: expected `{p}`, found {:?}", self.peek());
    }

    fn word(&mut self) -> String {
        match self.toks.get(self.pos) {
            Some(Tok::Word(w)) => {
                let w = w.clone();
                self.pos += 1;
                w
            }
            other => panic!("catalogue.bnf: expected a word, found {other:?}"),
        }
    }

    fn string(&mut self) -> String {
        match self.toks.get(self.pos) {
            Some(Tok::Str(s)) => {
                let s = s.clone();
                self.pos += 1;
                s
            }
            other => panic!("catalogue.bnf: expected a string literal, found {other:?}"),
        }
    }

    /// `( name: Type, name: Type+, name = Type ) -> Type`
    fn signature(&mut self) -> Signature {
        self.expect_punct("(");
        let mut params = Vec::new();
        if !self.eat_punct(")") {
            loop {
                let name = self.word();
                // The separator IS the named-ness: `on = Predicate` is written
                // with its name at the call site and `text: String` is not.
                let named = if self.eat_punct(":") {
                    false
                } else {
                    self.expect_punct("=");
                    true
                };
                let word = self.word();
                let ty = SigTy::parse(&word)
                    .unwrap_or_else(|| panic!("catalogue.bnf: `{word}` names no type"));
                let arity = if self.eat_punct("+") {
                    Arity::OneOrMore
                } else if self.eat_punct("?") {
                    Arity::Optional
                } else {
                    Arity::One
                };
                params.push(Param {
                    name,
                    ty,
                    arity,
                    named,
                });
                if self.eat_punct(",") {
                    continue;
                }
                self.expect_punct(")");
                break;
            }
        }
        self.expect_punct("->");
        let word = self.word();
        let ret =
            SigTy::parse(&word).unwrap_or_else(|| panic!("catalogue.bnf: `{word}` names no type"));
        Signature { params, ret }
    }

    /// `expr "<template>"` or `op <Variant>`.
    fn lowering(&mut self) -> Lowering {
        let kind = self.word();
        match kind.as_str() {
            "expr" => Lowering::Expr(self.string()),
            "op" => Lowering::Op(self.word()),
            other => panic!("catalogue.bnf: `{other}` is not a lowering; write `expr` or `op`"),
        }
    }
}

/// Parse `catalogue.bnf`.
///
/// Everything outside a statement is `(* … *)` commentary, which is where the
/// argument lives and which [`lex`] drops. A malformed statement is a panic and
/// not a skip: this is the source of truth now, so a line that does not parse is
/// a build failure rather than a name that quietly stops existing.
///
/// # Panics
///
/// On any statement that is not a well-formed `row` or `fn`, and on a file that
/// declares no rows at all.
#[must_use]
pub fn parse(text: &str) -> Catalogue {
    let mut p = Parser {
        toks: lex(text),
        pos: 0,
    };
    let mut cat = Catalogue::default();
    while p.peek().is_some() {
        let kw = p.word();
        match kw.as_str() {
            "row" => cat.rows.push(parse_row(&mut p)),
            "fn" => cat.fns.push(parse_fn(&mut p)),
            other => panic!("catalogue.bnf: `{other}` begins no statement; write `row` or `fn`"),
        }
    }
    assert!(
        !cat.rows.is_empty(),
        "catalogue.bnf declares no `io.` rows at all"
    );
    cat
}

/// `fn <dotted-name> = <signature> ; <lowering> .`
fn parse_fn(p: &mut Parser) -> FnRow {
    let name = p.word();
    assert!(
        name.contains('.'),
        "catalogue.bnf: `{name}` is not a dotted catalogue name"
    );
    p.expect_punct("=");
    let sig = p.signature();
    p.expect_punct(";");
    let lowering = p.lowering();
    p.expect_punct(".");
    FnRow {
        name,
        sig,
        lowering,
    }
}

/// `row <name> = extensions "…"+ ; <clause> [; signature <sig> ; <lowering>] .`
fn parse_row(p: &mut Parser) -> Row {
    let name = p.word();
    p.expect_punct("=");

    let mut extensions = Vec::new();
    let mut reads = None;
    let mut decodes = None;
    let mut call = None;

    loop {
        let clause = p.word();
        match clause.as_str() {
            "extensions" => {
                while matches!(p.peek(), Some(Tok::Str(_))) {
                    extensions.push(p.string());
                }
                assert!(
                    !extensions.is_empty(),
                    "row `{name}` declares no extension"
                );
            }
            "reads" => {
                let what = p.word();
                match what.as_str() {
                    "native" => reads = Some(Reads::Native(p.word())),
                    "materialised" => reads = Some(Reads::Materialised),
                    other => panic!("row `{name}`: unknown `reads` form `{other}`"),
                }
            }
            "decodes" => decodes = Some(p.word()),
            "signature" => {
                let sig = p.signature();
                p.expect_punct(";");
                call = Some((sig, p.lowering()));
            }
            other => panic!("row `{name}`: unknown clause `{other}`"),
        }
        if p.eat_punct(";") {
            continue;
        }
        p.expect_punct(".");
        break;
    }

    assert!(
        reads.is_some() || decodes.is_some(),
        "row `{name}` declares no capability at all"
    );
    Row {
        name,
        extensions,
        reads,
        decodes,
        call,
    }
}

// ── The emitters ───────────────────────────────────────────────────────────

/// The header every generated Rust file carries.
fn header(module_doc: &str) -> String {
    format!(
        "//! @generated by `cargo xtask catalogue` from `catalogue.bnf`. DO NOT EDIT.\n\
         //!\n\
         {module_doc}\n\
         //!\n\
         //! Edit `catalogue.bnf` and re-run `cargo xtask catalogue`.\n\n"
    )
}

/// A `&[\"a\", \"b\"]` literal.
fn extensions_literal(extensions: &[String]) -> String {
    let inner: Vec<String> = extensions.iter().map(|e| format!("{e:?}")).collect();
    format!("&[{}]", inner.join(", "))
}

/// Emit one `pub static NAME: Provider = …`.
fn emit_row(out: &mut String, row: &Row) {
    let reads_rows = match &row.reads {
        Some(Reads::Native(f)) => {
            format!("Some(RowReader::Native(NativeReader::{}))", variant_of(f))
        }
        Some(Reads::Materialised) => "Some(RowReader::Materialised)".to_owned(),
        None => "None".to_owned(),
    };
    let reads_types = match &row.decodes {
        Some(f) => format!("Some({f})"),
        None => "None".to_owned(),
    };
    let what = match (&row.reads, &row.decodes) {
        (Some(Reads::Native(f)), _) => format!("reads rows via `{f}`"),
        (Some(Reads::Materialised), _) => "reads rows, materialised outside the reader".to_owned(),
        (None, Some(f)) => format!("reads types via `{f}`"),
        (None, None) => unreachable!("parse rejects a row with no capability"),
    };

    let _ = writeln!(out, "/// `io.{}` — {what}.", row.name);
    let _ = writeln!(
        out,
        "pub static {}: Provider = Provider {{",
        row.static_ident()
    );
    let _ = writeln!(out, "    name: {:?},", row.name);
    let _ = writeln!(
        out,
        "    extensions: {},",
        extensions_literal(&row.extensions)
    );
    let _ = writeln!(out, "    reads_rows: {reads_rows},");
    let _ = writeln!(out, "    reads_types: {reads_types},");
    let _ = writeln!(out, "}};\n");
}

/// The `fossil-base` half: `NativeReader`, the rows the compiler can always
/// name, and `DATA`.
#[must_use]
pub fn emit_base(rows: &[Row]) -> String {
    let data: Vec<&Row> = rows.iter().filter(|r| !r.needs_a_decoder()).collect();

    let mut out = header(
        "//! The rows whose behaviour the compiler can always link: every row that\n\
         //! reads DATA. A row that decodes a shape language is emitted into\n\
         //! `fossil-descriptors-output` instead — see `xtask::catalogue`.",
    );
    out.push_str("use super::{Provider, RowReader};\n\n");

    // The reader enum, one variant per DISTINCT table function, in file order.
    let mut readers: Vec<&String> = Vec::new();
    for row in &data {
        if let Some(Reads::Native(f)) = &row.reads
            && !readers.contains(&f)
        {
            readers.push(f);
        }
    }

    out.push_str(
        "/// The native `DuckDB` readers a [`RowReader::Native`] maps to.\n\
         ///\n\
         /// `fossil-mir` exhaustively maps each to a `SourceFormat`, so a new reader is\n\
         /// a compile error until handled — which is the property generating this enum\n\
         /// from `catalogue.bnf` is meant to keep, not trade away.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n\
         pub enum NativeReader {\n",
    );
    for f in &readers {
        let _ = writeln!(out, "    /// `{f}`.");
        let _ = writeln!(out, "    {},", variant_of(f));
    }
    out.push_str("}\n\n");

    out.push_str(
        "impl NativeReader {\n\
         \x20   /// The `DuckDB` table function this reader names — the token\n\
         \x20   /// `catalogue.bnf` writes after `native`.\n\
         \x20   #[must_use]\n\
         \x20   pub const fn table_function(self) -> &'static str {\n\
         \x20       match self {\n",
    );
    for f in &readers {
        let _ = writeln!(out, "            Self::{} => {f:?},", variant_of(f));
    }
    out.push_str("        }\n    }\n}\n\n");

    for row in &data {
        emit_row(&mut out, row);
    }

    out.push_str(
        "/// The rows the compiler can name on its own: everything that reads DATA.\n\
         ///\n\
         /// This is [`crate::system::System::providers`]'s default, and it is a real\n\
         /// answer rather than a stub — a host that decodes no shape document still has\n\
         /// to recognise `io.csv`. A host that compiles programs installs a superset\n\
         /// (`fossil_descriptors_output::PROVIDERS`), and the rows it adds are exactly\n\
         /// the ones `catalogue.bnf` gives a `decodes`.\n",
    );
    let refs: Vec<String> = data
        .iter()
        .map(|r| format!("&{}", r.static_ident()))
        .collect();
    let _ = writeln!(
        out,
        "pub static DATA: &[&Provider] = &[{}];",
        refs.join(", ")
    );
    out
}

/// The `fossil-hir` half: the stdlib catalogue as a table of `RegistryEntry`.
///
/// `RegistryEntry::new` derives the receiver and the member from the dotted
/// name, so this emitter never writes one — the property
/// `every_rows_receiver_agrees_with_its_own_name` guards survives generation
/// because the generator cannot express its violation.
///
/// A `PlanOp` variant is emitted as a Rust PATH for the reason `decodes` is: an
/// `op` token naming a variant nobody wrote is a build failure. `PlanOp` itself
/// stays hand-written in `fossil-hir`, because `lower_source_stage` matches it
/// exhaustively and each variant carries what it lowers to — prose a `.bnf`
/// token has nowhere to put.
#[must_use]
pub fn emit_hir_stdlib(cat: &Catalogue) -> String {
    let mut out = header(
        "//! The stdlib catalogue: every function the language declares, with its\n\
         //! signature and how it compiles. `crate::stdlib` owns the TYPES this\n\
         //! table is written in, and `FunctionRegistry::stdlib_default` is its only\n\
         //! consumer.",
    );
    out.push_str("use smol_str::SmolStr;\n\n");
    out.push_str(
        "use super::{Arity, LoweringKind, ParamSpec, PlanOp, RegistryEntry, ScalarTy, SigTy};\n\n",
    );
    out.push_str(
        "/// Every catalogue row, in `catalogue.bnf` order.\n\
         pub(super) fn rows() -> Vec<RegistryEntry> {\n\
         \x20   vec![\n",
    );
    for (name, sig, lowering) in cat.registry_rows() {
        let params: Vec<String> = sig
            .params
            .iter()
            .map(|p| {
                format!(
                    "ParamSpec {{ name: SmolStr::new({:?}), ty: {}, arity: {}, named: {} }}",
                    p.name,
                    p.ty.rust_path(),
                    p.arity.rust_path(),
                    p.named
                )
            })
            .collect();
        let low = match lowering {
            Lowering::Expr(t) => format!("LoweringKind::Expr(SmolStr::new({t:?}))"),
            Lowering::Op(v) => format!("LoweringKind::Op(PlanOp::{v})"),
        };
        let _ = writeln!(
            out,
            "        RegistryEntry::new({:?}, vec![{}], {}, {}),",
            name,
            params.join(", "),
            sig.ret.rust_path(),
            low
        );
    }
    out.push_str("    ]\n}\n");
    out
}

/// The rows that read data through a native `DuckDB` table function, paired with
/// that function's name — the projection both TypeScript halves want.
fn native_rows(rows: &[Row]) -> Vec<(&str, &str)> {
    rows.iter()
        .filter_map(|r| match &r.reads {
            Some(Reads::Native(f)) => Some((r.name.as_str(), f.as_str())),
            _ => None,
        })
        .collect()
}

/// The header for a generated TypeScript module.
fn ts_header(body: &str) -> String {
    format!(
        "// @generated by `cargo xtask catalogue` from `catalogue.bnf`. DO NOT EDIT.\n\
         //\n\
         {body}\n\
         //\n\
         // Edit `catalogue.bnf` and re-run `cargo xtask catalogue`.\n\n"
    )
}

/// `@fossil-lang/introspect`: the constructors an introspecting host can
/// `DESCRIBE`, and the reader each one goes through.
///
/// A materialised row (`io.rdf`) is absent on purpose and not by omission —
/// there is no table function to `DESCRIBE` it with, which is what
/// `reads materialised` means.
#[must_use]
pub fn emit_ts_introspect(rows: &[Row]) -> String {
    let native = native_rows(rows);
    let mut out = ts_header(
        "// The `io.` constructors that read through a native DuckDB table function.\n\
         // A materialised row (`io.rdf`) has no reader to DESCRIBE through and is\n\
         // deliberately absent — `packages/executor` gets the list that includes it.",
    );

    out.push_str("/** The DuckDB table function each constructor reads through. */\n");
    out.push_str("export const NATIVE_READERS = {\n");
    for (name, f) in &native {
        let _ = writeln!(out, "  {name}: {f:?},");
    }
    out.push_str("} as const;\n\n");

    out.push_str("/** The constructors above, in catalogue order — the alternation's corpus. */\n");
    let names: Vec<String> = native.iter().map(|(n, _)| format!("{n:?}")).collect();
    let _ = writeln!(
        out,
        "export const NATIVE_ROWS = [{}] as const;\n",
        names.join(", ")
    );

    out.push_str("/** One `io.` constructor that reads through a native reader. */\n");
    out.push_str("export type NativeRow = (typeof NATIVE_ROWS)[number];\n");
    out
}

/// `@fossil-lang/executor`: every constructor that reads DATA — the fetch
/// strategies a host has to stage bytes for, `io.rdf` included.
#[must_use]
pub fn emit_ts_executor(rows: &[Row]) -> String {
    let data: Vec<&str> = rows
        .iter()
        .filter(|r| r.reads.is_some())
        .map(|r| r.name.as_str())
        .collect();

    let mut out = ts_header(
        "// Every `io.` constructor that reads DATA. This is the wire vocabulary of\n\
         // `FossilExecutor.sources()` and `SourceInput.format`: the string IS the\n\
         // catalogue row's name, and `fossil-df-wasm` reads it back with\n\
         // `source_row`. Unlike the introspect list, `io.rdf` is here — the host\n\
         // fetches its bytes like any other source; only the staging differs.",
    );
    out.push_str("/** Every `io.` constructor that reads data, in catalogue order. */\n");
    let names: Vec<String> = data.iter().map(|n| format!("{n:?}")).collect();
    let _ = writeln!(
        out,
        "export const DATA_ROWS = [{}] as const;\n",
        names.join(", ")
    );
    out.push_str("/** The fetch strategy for a source — how the host must stage its bytes. */\n");
    out.push_str("export type DataRow = (typeof DATA_ROWS)[number];\n");
    out
}

/// The `fossil-descriptors-output` half: the rows that decode a shape language,
/// and the assembled table.
#[must_use]
pub fn emit_descriptors(rows: &[Row]) -> String {
    let typed: Vec<&Row> = rows.iter().filter(|r| r.needs_a_decoder()).collect();

    let mut out = header(
        "//! The rows that read TYPES, and [`PROVIDERS`] — every row in\n\
         //! `catalogue.bnf`, which is what a COMPILING host installs.\n\
         //!\n\
         //! Each `decodes` token below is a Rust path, so the compiler resolves it.\n\
         //! That is what the parity test this replaces said it could not prove: a `fn`\n\
         //! pointer has no name at run time, and a row could declare a capability whose\n\
         //! function nobody had written.",
    );
    out.push_str("use fossil_base::Provider;\n");
    out.push_str("use fossil_base::providers::{");
    let data_idents: Vec<String> = rows
        .iter()
        .filter(|r| !r.needs_a_decoder())
        .map(Row::static_ident)
        .collect();
    out.push_str(&data_idents.join(", "));
    out.push_str("};\n\n");

    let decoders: Vec<String> = typed
        .iter()
        .filter_map(|r| r.decodes.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let _ = writeln!(out, "use super::{{{}}};\n", decoders.join(", "));

    for row in &typed {
        emit_row(&mut out, row);
    }

    out.push_str(
        "/// **The registry a compiling host installs**: every row `catalogue.bnf`\n\
         /// declares, in file order.\n\
         ///\n\
         /// A host that returns this from `System::providers` recognises every `io.*`\n\
         /// the language has. The default (`fossil_base::providers::DATA`) is the data\n\
         /// rows alone, which is correct for a host that runs a plan somebody else\n\
         /// compiled and wrong for the host the plan comes from.\n",
    );
    let refs: Vec<String> = rows
        .iter()
        .map(|r| format!("&{}", r.static_ident()))
        .collect();
    let _ = writeln!(
        out,
        "pub static PROVIDERS: &[&Provider] = &[{}];",
        refs.join(", ")
    );
    out
}

/// Run the source text through `rustfmt`, so the generated files are formatted
/// the way `cargo fmt --check` demands rather than the way this emitter happens
/// to write them.
///
/// # Panics
///
/// If `rustfmt` is missing or rejects the emitted source — the second is a bug
/// in this emitter and should fail loudly rather than commit unformatted Rust.
#[must_use]
pub fn rustfmt(source: &str) -> String {
    use std::io::Write as _;

    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn rustfmt");
    child
        .stdin
        .take()
        .expect("rustfmt stdin")
        .write_all(source.as_bytes())
        .expect("write to rustfmt");
    let out = child.wait_with_output().expect("wait for rustfmt");
    assert!(
        out.status.success(),
        "rustfmt rejected the generated source"
    );
    String::from_utf8(out.stdout).expect("rustfmt emits utf-8")
}

/// The repository root — the directory holding `catalogue.bnf`.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask lives two directories below the repo root")
        .to_path_buf()
}

/// Parse `catalogue.bnf` off disk.
///
/// # Panics
///
/// If the file cannot be read or does not parse.
#[must_use]
pub fn read() -> Catalogue {
    let text =
        std::fs::read_to_string(repo_root().join("catalogue.bnf")).expect("read catalogue.bnf");
    parse(&text)
}

/// Every generated file: its path, and what it should contain.
///
/// All six are projections of `catalogue.bnf` and of nothing else. The fifth —
/// `fossil-hir`'s stdlib table — is why the sixth, the reference page, no longer
/// needs a second source: both halves of the catalogue are in the file now, so
/// `crate::reference` reads the same parse everything else does.
///
/// # Panics
///
/// If `catalogue.bnf` cannot be read or does not parse.
#[must_use]
pub fn generated() -> Vec<(PathBuf, String)> {
    let root = repo_root();
    let cat = read();
    let rows = &cat.rows;
    vec![
        (
            root.join("crates/fossil-base/src/providers/generated.rs"),
            rustfmt(&emit_base(rows)),
        ),
        (
            root.join("crates/fossil-descriptors-output/src/generated.rs"),
            rustfmt(&emit_descriptors(rows)),
        ),
        (
            root.join("crates/fossil-hir/src/stdlib/generated.rs"),
            rustfmt(&emit_hir_stdlib(&cat)),
        ),
        (
            root.join("packages/introspect/src/catalogue.generated.ts"),
            emit_ts_introspect(rows),
        ),
        (
            root.join("packages/executor/src/catalogue.generated.ts"),
            emit_ts_executor(rows),
        ),
        (
            root.join(crate::reference::PARTIAL),
            crate::reference::emit_stdlib_reference(&cat),
        ),
    ]
}
